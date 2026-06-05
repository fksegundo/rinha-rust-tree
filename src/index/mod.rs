pub mod build;
pub mod format;
mod layout;
mod mmap;
pub mod partition_scheme;
mod search;

#[cfg(test)]
mod tests;

use crate::{DIMS, K, PACKED_DIMS, QueryVector, SCALE};
use std::mem::{self, MaybeUninit};
use std::slice;

use mmap::MmapRegion;
use search::{
    PendingSubtrees, align_cursor, insert_best, insert_best_fast, lower_bound_box,
    query_pairs_avx2, read_i16, read_i32, read_u8, replay_pending_if_needed,
    scan_block_pair_avx2_bounded, sort_partition_entries,
};
const MAGIC_V5: &[u8; 8] = b"RNSPCST5";
const LANES: usize = 8;
const KEY_LOOKUP_SIZE: usize = 1024;
const MAX_PARTITIONS: usize = KEY_LOOKUP_SIZE;
const TREE_STACK_CAPACITY: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IndexFormat {
    V5,
}

pub struct SpecialistIndex {
    _mapping: MmapRegion,
    reference_count: usize,
    partitions_base: *const u8,
    partition_count: usize,
    key_to_partition: [i32; KEY_LOOKUP_SIZE],
    active_keys: Vec<u32>,
    partition_scheme: partition_scheme::PartitionScheme,
    nodes_base: *const u8,
    node_count: usize,
    vectors: *const i16,
    vectors_len: usize,
    labels: *const u8,
    labels_len: usize,
    ref_indices: *const u32,
    ref_indices_len: usize,
    node_class_bits: *const u8,
    node_class_bits_len: usize,
    early_exit_threshold: std::sync::atomic::AtomicI64,
    label_defer: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IndexMetadata {
    pub reference_count: usize,
    pub partition_count: usize,
    pub node_count: usize,
    pub block_count: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SearchStats {
    pub partitions_visited: u32,
    pub secondary_partitions: u32,
    pub nodes_visited: u32,
    pub leaves_scanned: u32,
    pub blocks_scanned: u32,
}

/// Bitset over partition keys (0..1024) for Modo B routing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PartitionSet(pub [u64; 16]);

impl PartitionSet {
    pub const fn empty() -> Self {
        Self([0; 16])
    }

    #[inline]
    pub fn set(&mut self, key: u32) {
        let k = key as usize;
        if k < KEY_LOOKUP_SIZE {
            self.0[k / 64] |= 1u64 << (k % 64);
        }
    }

    #[inline]
    pub fn contains(&self, key: u32) -> bool {
        let k = key as usize;
        if k >= KEY_LOOKUP_SIZE {
            return false;
        }
        (self.0[k / 64] & (1u64 << (k % 64))) != 0
    }

    pub fn from_top_keys(keys: &[u32; K]) -> Self {
        let mut s = Self::empty();
        for &k in keys {
            if k < KEY_LOOKUP_SIZE as u32 {
                s.set(k);
            }
        }
        s
    }

    pub fn keys_sorted(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for k in 0..KEY_LOOKUP_SIZE as u32 {
            if self.contains(k) {
                out.push(k);
            }
        }
        out
    }
}

unsafe impl Send for SpecialistIndex {}
unsafe impl Sync for SpecialistIndex {}

impl SpecialistIndex {
    pub fn open(path: &str) -> Result<Self, String> {
        let mapping = MmapRegion::open(path)?;
        let bytes = mapping.as_slice();
        if bytes.len() < 8 {
            return Err("file too short".to_string());
        }
        let magic: &[u8; 8] = bytes[..8].try_into().unwrap();
        let format = if magic == MAGIC_V5 {
            IndexFormat::V5
        } else {
            return Err(format!(
                "unsupported index magic: {:?}. Rebuild index with the preprocess binary",
                magic
            ));
        };
        Self::load(mapping, format)
    }

    fn load(mapping: MmapRegion, format: IndexFormat) -> Result<Self, String> {
        let bytes = mapping.as_slice();
        let mut cursor = 8usize;

        let scale = read_i32(bytes, &mut cursor)?;
        let packed_dims = read_i32(bytes, &mut cursor)? as usize;
        let reference_count = read_i32(bytes, &mut cursor)? as usize;
        let partition_count = read_i32(bytes, &mut cursor)? as usize;
        let node_count = read_i32(bytes, &mut cursor)? as usize;
        let total_blocks = read_i32(bytes, &mut cursor)? as usize;

        let scheme_id = read_i16(bytes, &mut cursor)?;
        let scheme_param = read_i16(bytes, &mut cursor)? as usize;
        let amount_cut_count = read_i16(bytes, &mut cursor)? as usize;
        let dow_cut_count = read_i16(bytes, &mut cursor)? as usize;
        let predicate_count = read_i16(bytes, &mut cursor)? as usize;

        // Legacy cuts: now always 0, but we still skip the bytes to keep the
        // header offsets stable.
        for _ in 0..(amount_cut_count + dow_cut_count) {
            read_i16(bytes, &mut cursor)?;
        }

        let mut tree_predicates = Vec::with_capacity(predicate_count);
        for _ in 0..predicate_count {
            let dim = read_u8(bytes, &mut cursor)?;
            let flags = read_u8(bytes, &mut cursor)?;
            let threshold = read_i16(bytes, &mut cursor)?;
            let enabled = flags != 0;
            if enabled && dim as usize >= DIMS {
                return Err(format!("invalid tree predicate dimension: {dim}"));
            }
            tree_predicates.push(partition_scheme::TreePredicate {
                dim,
                threshold,
                enabled,
            });
        }

        let partition_scheme = partition_scheme::PartitionScheme::from_header(
            scheme_id,
            scheme_param,
            tree_predicates,
        )?;

        if scale != SCALE as i32 {
            return Err(format!(
                "invalid index scale: expected {}, got {}",
                SCALE, scale
            ));
        }

        if packed_dims != PACKED_DIMS {
            return Err("invalid packed dimensions".to_string());
        }

        let partitions_base = unsafe { bytes.as_ptr().add(cursor) };
        let partitions_bytes = partition_count
            .checked_mul(layout::PARTITION_STRIDE)
            .ok_or_else(|| "partition_count overflow".to_string())?;
        if cursor + partitions_bytes > bytes.len() {
            return Err("truncated partitions".to_string());
        }
        cursor += partitions_bytes;

        // Map partition key -> partition index (in-file order) (key-first search).
        let mut key_to_partition = [-1i32; KEY_LOOKUP_SIZE];
        for idx in 0..partition_count {
            let key = unsafe { layout::partition_key(partitions_base, idx) } as usize;
            if key < KEY_LOOKUP_SIZE {
                key_to_partition[key] = idx as i32;
            }
        }

        let nodes_base = unsafe { bytes.as_ptr().add(cursor) };
        let nodes_bytes = node_count
            .checked_mul(layout::NODE_STRIDE)
            .ok_or_else(|| "node_count overflow".to_string())?;
        if cursor + nodes_bytes > bytes.len() {
            return Err("truncated nodes".to_string());
        }
        cursor += nodes_bytes;

        let vectors_len = total_blocks * DIMS * LANES;
        let vectors_bytes = vectors_len * mem::size_of::<i16>();
        if cursor % mem::align_of::<i16>() != 0 {
            return Err("unaligned vectors section".to_string());
        }
        if cursor + vectors_bytes > bytes.len() {
            return Err("truncated vectors".to_string());
        }
        let vectors = unsafe { bytes.as_ptr().add(cursor).cast::<i16>() };
        cursor += vectors_bytes;

        let labels_len = total_blocks * LANES;
        if cursor + labels_len > bytes.len() {
            return Err("truncated labels".to_string());
        }
        let labels = unsafe { bytes.as_ptr().add(cursor) };
        cursor += labels_len;

        cursor = align_cursor(cursor, mem::align_of::<u32>());
        let ref_indices_len = total_blocks * LANES;
        let ref_indices_bytes = ref_indices_len * mem::size_of::<u32>();
        if cursor % mem::align_of::<u32>() != 0 {
            return Err("unaligned ref_indices section".to_string());
        }
        if cursor + ref_indices_bytes > bytes.len() {
            return Err("truncated ref indices".to_string());
        }
        let ref_indices = unsafe { bytes.as_ptr().add(cursor).cast::<u32>() };
        cursor += ref_indices_bytes;

        let node_class_bits_len = node_count;
        if cursor + node_class_bits_len > bytes.len() {
            return Err("truncated node class bits".to_string());
        }
        let node_class_bits = unsafe { bytes.as_ptr().add(cursor) };

        let early_exit_threshold_val = std::env::var("RINHA_EARLY_EXIT_THRESHOLD")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        let early_exit_threshold = std::sync::atomic::AtomicI64::new(early_exit_threshold_val);
        let label_defer = std::env::var("RINHA_LABEL_DEFER")
            .map(|value| value != "0")
            .unwrap_or(false);

        eprintln!(
            "[{:?}] loaded: {} partitions, {} nodes, {} blocks, avx2=true, early_exit={}, label_defer={}, tree_depth={}, tree_predicates={}",
            format,
            partition_count,
            node_count,
            total_blocks,
            early_exit_threshold_val,
            label_defer,
            partition_scheme.tree_depth,
            partition_scheme.tree_predicates.len()
        );

        let mut active_keys = Vec::with_capacity(partition_count);
        for (key, &idx) in key_to_partition.iter().enumerate() {
            if idx >= 0 {
                active_keys.push(key as u32);
            }
        }

        let index = Self {
            _mapping: mapping,
            reference_count,
            partitions_base,
            partition_count,
            key_to_partition,
            active_keys,
            partition_scheme,
            nodes_base,
            node_count,
            vectors,
            vectors_len,
            labels,
            labels_len,
            ref_indices,
            ref_indices_len,
            node_class_bits,
            node_class_bits_len,
            early_exit_threshold,
            label_defer,
        };
        index.advise_hugepages();
        Ok(index)
    }

    #[inline]
    pub fn compute_partition_key(&self, vector: &QueryVector) -> u32 {
        self.partition_scheme.compute_key(vector)
    }

    pub fn partition_key_bits(&self) -> usize {
        self.partition_scheme.key_bits()
    }

    pub fn predict_fraud_count(&self, query: &QueryVector) -> u8 {
        let mut best_dists = [i64::MAX; K];
        let mut best_labels = [0u8; K];
        let mut best_indices = [u32::MAX; K];
        let mut pending_subtrees = PendingSubtrees::new(self.label_defer);

        let query_key = self.compute_partition_key(query);
        let eet = self
            .early_exit_threshold
            .load(std::sync::atomic::Ordering::Relaxed);

        if let Some(idx) = self.partition_idx_for_key(query_key) {
            let root = unsafe { layout::partition_root(self.partitions_base, idx) };
            let bound = lower_bound_box(
                query,
                unsafe { layout::partition_min(self.partitions_base, idx) },
                unsafe { layout::partition_max(self.partitions_base, idx) },
            );
            self.search_node_iterative_fast(
                root,
                bound,
                query,
                &mut best_dists,
                &mut best_labels,
                &mut best_indices,
                Some(&mut pending_subtrees),
            );
            replay_pending_if_needed(
                &mut pending_subtrees,
                &mut best_dists,
                &mut best_labels,
                |root, bound, best_dists, best_labels| {
                    self.search_node_iterative_fast(
                        root,
                        bound,
                        query,
                        best_dists,
                        best_labels,
                        &mut best_indices,
                        None,
                    );
                },
            );
            if eet > 0 && best_dists[K - 1] < eet {
                return best_labels.iter().map(|&l| l as u32).sum::<u32>() as u8;
            }
        }

        let mut partition_entries: MaybeUninit<[(i64, usize); MAX_PARTITIONS]> =
            MaybeUninit::uninit();
        let partition_entries_ptr = partition_entries.as_mut_ptr();
        let mut partition_len = 0usize;

        for &key in &self.active_keys {
            if key == query_key {
                continue;
            }
            let idx = self.key_to_partition[key as usize] as usize;
            let bound = lower_bound_box(
                query,
                unsafe { layout::partition_min(self.partitions_base, idx) },
                unsafe { layout::partition_max(self.partitions_base, idx) },
            );
            if bound < best_dists[K - 1] {
                unsafe {
                    (*partition_entries_ptr)[partition_len] = (bound, idx);
                }
                partition_len += 1;
            }
        }

        let partition_entries_slice = unsafe {
            std::slice::from_raw_parts_mut(
                partition_entries_ptr as *mut (i64, usize),
                partition_len,
            )
        };
        sort_partition_entries(partition_entries_slice);

        for i in 0..partition_len {
            let (bound, idx) = partition_entries_slice[i];
            if bound >= best_dists[K - 1] {
                break;
            }
            self.search_node_iterative_fast(
                unsafe { layout::partition_root(self.partitions_base, idx) },
                bound,
                query,
                &mut best_dists,
                &mut best_labels,
                &mut best_indices,
                Some(&mut pending_subtrees),
            );
            replay_pending_if_needed(
                &mut pending_subtrees,
                &mut best_dists,
                &mut best_labels,
                |root, bound, best_dists, best_labels| {
                    self.search_node_iterative_fast(
                        root,
                        bound,
                        query,
                        best_dists,
                        best_labels,
                        &mut best_indices,
                        None,
                    );
                },
            );
            if eet > 0 && best_dists[K - 1] < eet {
                break;
            }
        }

        best_labels.iter().map(|&l| l as u32).sum::<u32>() as u8
    }

    fn search_node_iterative_fast(
        &self,
        root: usize,
        root_bound: i64,
        query: &QueryVector,
        best_dists: &mut [i64; K],
        best_labels: &mut [u8; K],
        best_indices: &mut [u32; K],
        mut pending_subtrees: Option<&mut PendingSubtrees>,
    ) {
        let mut stack_nodes = [0usize; TREE_STACK_CAPACITY];
        let mut stack_bounds = [0i64; TREE_STACK_CAPACITY];
        let mut stack_len = 0usize;

        let mut current = root;
        let mut current_bound = root_bound;

        loop {
            if current_bound <= best_dists[K - 1] {
                if let Some(pending) = pending_subtrees.as_deref_mut()
                    && pending.try_defer(
                        current,
                        current_bound,
                        best_dists,
                        best_labels,
                        self.node_class_bits(current),
                    )
                {
                    if stack_len == 0 {
                        break;
                    }
                    stack_len -= 1;
                    current = stack_nodes[stack_len];
                    current_bound = stack_bounds[stack_len];
                    continue;
                }
                let left = unsafe { layout::node_left(self.nodes_base, current) };
                let right = unsafe { layout::node_right(self.nodes_base, current) };
                if left < 0 || right < 0 {
                    self.scan_leaf_fast(current, query, best_dists, best_labels, best_indices);
                } else {
                    let l = left as usize;
                    let r = right as usize;

                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        use std::arch::x86_64::*;
                        _mm_prefetch(
                            self.nodes_base.add(r * layout::NODE_STRIDE) as *const i8,
                            _MM_HINT_T0,
                        );
                    }

                    let lb = lower_bound_box(
                        query,
                        unsafe { layout::node_min(self.nodes_base, l) },
                        unsafe { layout::node_max(self.nodes_base, l) },
                    );
                    let rb = lower_bound_box(
                        query,
                        unsafe { layout::node_min(self.nodes_base, r) },
                        unsafe { layout::node_max(self.nodes_base, r) },
                    );

                    let (near_idx, near_bound, far_idx, far_bound) = if lb <= rb {
                        (l, lb, r, rb)
                    } else {
                        (r, rb, l, lb)
                    };

                    if far_bound <= best_dists[K - 1] && stack_len < TREE_STACK_CAPACITY {
                        stack_nodes[stack_len] = far_idx;
                        stack_bounds[stack_len] = far_bound;
                        stack_len += 1;
                    }

                    if near_bound <= best_dists[K - 1] {
                        current = near_idx;
                        current_bound = near_bound;
                        continue;
                    }
                }
            }

            if stack_len == 0 {
                break;
            }

            stack_len -= 1;
            current = stack_nodes[stack_len];
            current_bound = stack_bounds[stack_len];
        }
    }

    fn scan_leaf_fast(
        &self,
        node_idx: usize,
        query: &QueryVector,
        best_dists: &mut [i64; K],
        best_labels: &mut [u8; K],
        best_indices: &mut [u32; K],
    ) {
        let start_block = unsafe { layout::node_start(self.nodes_base, node_idx) };
        let node_len = unsafe { layout::node_len(self.nodes_base, node_idx) };
        let blocks = (node_len + LANES - 1) / LANES;
        let vectors = self.vectors();
        let labels = self.labels();
        let ref_indices = self.ref_indices();
        let q_pairs = unsafe { query_pairs_avx2(query) };

        for b in 0..blocks {
            let block_idx = start_block + b;
            let block_base = block_idx * DIMS * LANES;

            #[cfg(target_arch = "x86_64")]
            if b + 1 < blocks {
                unsafe {
                    use std::arch::x86_64::*;
                    let next_base = (start_block + b + 1) * DIMS * LANES;
                    let ptr = self.vectors.add(next_base) as *const i8;
                    _mm_prefetch(ptr, _MM_HINT_T0);
                    _mm_prefetch(ptr.add(64), _MM_HINT_T0);
                    _mm_prefetch(ptr.add(128), _MM_HINT_T0);
                    _mm_prefetch(ptr.add(192), _MM_HINT_T0);

                    let labels_ptr = self.labels.add((start_block + b + 1) * LANES) as *const i8;
                    _mm_prefetch(labels_ptr, _MM_HINT_T0);
                }
            }

            let (mask, dists) = unsafe {
                scan_block_pair_avx2_bounded(vectors, block_base, &q_pairs, best_dists[K - 1])
            };
            if mask == 0 {
                continue;
            }
            let labels_base = block_idx * LANES;
            let lane_count = (node_len - b * LANES).min(LANES);
            let mut mask = mask & ((1u32 << lane_count) - 1);
            while mask != 0 {
                let i = mask.trailing_zeros() as usize;
                mask &= mask - 1;
                insert_best_fast(
                    dists[i] as i64,
                    labels[labels_base + i],
                    ref_indices[labels_base + i],
                    best_dists,
                    best_labels,
                    best_indices,
                );
            }
        }
    }

    pub fn predict_fraud_count_with_stats(&self, query: &QueryVector) -> (u8, SearchStats) {
        let mut stats = SearchStats::default();
        let count = self.predict_fraud_count_inner(query, Some(&mut stats), None, None);
        (count, stats)
    }

    /// Full exact k-NN plus partition keys that contributed to the final top-5.
    pub fn predict_fraud_count_with_partitions(&self, query: &QueryVector) -> (u8, PartitionSet) {
        let mut part_keys = [u32::MAX; K];
        let count = self.predict_fraud_count_inner(query, None, None, Some(&mut part_keys));
        let set = PartitionSet::from_top_keys(&part_keys);
        (count, set)
    }

    /// Exact k-NN restricted to partitions in `allowed` (Modo B).
    pub fn predict_fraud_count_in_partitions(
        &self,
        query: &QueryVector,
        allowed: &PartitionSet,
    ) -> u8 {
        self.predict_fraud_count_inner(query, None, Some(allowed), None)
    }

    pub fn metadata(&self) -> IndexMetadata {
        IndexMetadata {
            reference_count: self.reference_count,
            partition_count: self.partition_count,
            node_count: self.node_count,
            block_count: self.vectors_len / (DIMS * LANES),
        }
    }

    pub fn mlock_all(&self) {
        #[cfg(target_os = "linux")]
        unsafe {
            let ptr = self._mapping.ptr as *mut libc::c_void;
            let len = self._mapping.len;
            if libc::mlock(ptr, len) != 0 {
                eprintln!(
                    "mlock({} bytes) failed: {}",
                    len,
                    std::io::Error::last_os_error()
                );
            } else {
                eprintln!("mlock({} bytes) succeeded", len);
            }
        }
    }

    pub fn pretouch_all(&self) {
        let bytes = self._mapping.as_slice();
        let mut checksum = 0u8;
        let mut offset = 0usize;
        while offset < bytes.len() {
            checksum ^= unsafe { std::ptr::read_volatile(bytes.as_ptr().add(offset)) };
            offset += 4096;
        }
        if let Some(last) = bytes.last() {
            checksum ^= unsafe { std::ptr::read_volatile(last) };
        }
        eprintln!(
            "pretouched index mapping ({} bytes, checksum={})",
            bytes.len(),
            checksum
        );
    }

    #[inline]
    fn partition_search_allowed(&self, key: u32, allowed: Option<&PartitionSet>) -> bool {
        match allowed {
            None => true,
            Some(set) => set.contains(key),
        }
    }

    fn predict_fraud_count_inner(
        &self,
        query: &QueryVector,
        mut stats: Option<&mut SearchStats>,
        allowed: Option<&PartitionSet>,
        mut track_part_keys: Option<&mut [u32; K]>,
    ) -> u8 {
        let mut best_dists = [i64::MAX; K];
        let mut best_labels = [0u8; K];
        let mut best_indices = [u32::MAX; K];
        if let Some(keys) = track_part_keys.as_mut() {
            keys.fill(u32::MAX);
        }

        let query_key = self.compute_partition_key(query);
        // Restricted search must be exact (no early exit).
        let eet = if allowed.is_some() {
            0
        } else {
            self.early_exit_threshold
                .load(std::sync::atomic::Ordering::Relaxed)
        };

        if let Some(idx) = self.partition_idx_for_key(query_key) {
            let part_key = unsafe { layout::partition_key(self.partitions_base, idx) };
            if self.partition_search_allowed(part_key, allowed) {
                let root = unsafe { layout::partition_root(self.partitions_base, idx) };
                let bound = lower_bound_box(
                    query,
                    unsafe { layout::partition_min(self.partitions_base, idx) },
                    unsafe { layout::partition_max(self.partitions_base, idx) },
                );
                self.search_node_iterative(
                    root,
                    bound,
                    query,
                    part_key,
                    &mut best_dists,
                    &mut best_labels,
                    &mut best_indices,
                    &mut stats,
                    &mut track_part_keys,
                );
                if eet > 0 && best_dists[K - 1] < eet {
                    return best_labels.iter().map(|&l| l as u32).sum::<u32>() as u8;
                }
            }
        }

        let mut partition_entries: MaybeUninit<[(i64, usize); MAX_PARTITIONS]> =
            MaybeUninit::uninit();
        let partition_entries_ptr = partition_entries.as_mut_ptr();
        let mut partition_len = 0usize;

        let allowed_keys;
        let candidate_keys: &[u32] = if let Some(allowed) = allowed {
            let (keys, len) = self.allowed_active_keys(allowed);
            allowed_keys = keys;
            &allowed_keys[..len]
        } else {
            &self.active_keys
        };

        for &key in candidate_keys {
            if key == query_key {
                continue;
            }
            let idx = self.key_to_partition[key as usize] as usize;
            let bound = lower_bound_box(
                query,
                unsafe { layout::partition_min(self.partitions_base, idx) },
                unsafe { layout::partition_max(self.partitions_base, idx) },
            );
            if bound < best_dists[K - 1] {
                unsafe {
                    (*partition_entries_ptr)[partition_len] = (bound, idx);
                }
                partition_len += 1;
            }
        }

        let partition_entries_slice = unsafe {
            std::slice::from_raw_parts_mut(
                partition_entries_ptr as *mut (i64, usize),
                partition_len,
            )
        };
        sort_partition_entries(partition_entries_slice);

        for i in 0..partition_len {
            let (bound, idx) = partition_entries_slice[i];
            if bound >= best_dists[K - 1] {
                break;
            }
            let part_key = unsafe { layout::partition_key(self.partitions_base, idx) };
            if let Some(s) = stats.as_deref_mut() {
                s.secondary_partitions += 1;
            }
            self.search_node_iterative(
                unsafe { layout::partition_root(self.partitions_base, idx) },
                bound,
                query,
                part_key,
                &mut best_dists,
                &mut best_labels,
                &mut best_indices,
                &mut stats,
                &mut track_part_keys,
            );
            if eet > 0 && best_dists[K - 1] < eet {
                break;
            }
        }

        best_labels.iter().map(|&l| l as u32).sum::<u32>() as u8
    }

    #[inline(always)]
    fn partition_idx_for_key(&self, key: u32) -> Option<usize> {
        let idx = self
            .key_to_partition
            .get(key as usize)
            .copied()
            .unwrap_or(-1);
        if idx >= 0 { Some(idx as usize) } else { None }
    }

    fn allowed_active_keys(&self, allowed: &PartitionSet) -> ([u32; MAX_PARTITIONS], usize) {
        let mut keys = [u32::MAX; MAX_PARTITIONS];
        let mut len = 0usize;
        for key in 0..KEY_LOOKUP_SIZE {
            if allowed.contains(key as u32) && self.key_to_partition[key] >= 0 {
                keys[len] = key as u32;
                len += 1;
            }
        }
        (keys, len)
    }

    fn search_node_iterative(
        &self,
        root: usize,
        root_bound: i64,
        query: &QueryVector,
        partition_key: u32,
        best_dists: &mut [i64; K],
        best_labels: &mut [u8; K],
        best_indices: &mut [u32; K],
        stats: &mut Option<&mut SearchStats>,
        track_part_keys: &mut Option<&mut [u32; K]>,
    ) {
        if let Some(s) = stats.as_deref_mut() {
            s.partitions_visited += 1;
        }

        let mut stack_nodes = [0usize; TREE_STACK_CAPACITY];
        let mut stack_bounds = [0i64; TREE_STACK_CAPACITY];
        let mut stack_len = 0usize;

        let mut current = root;
        let mut current_bound = root_bound;

        loop {
            if current_bound <= best_dists[K - 1] {
                if let Some(s) = stats.as_deref_mut() {
                    s.nodes_visited += 1;
                }
                let left = unsafe { layout::node_left(self.nodes_base, current) };
                let right = unsafe { layout::node_right(self.nodes_base, current) };
                if left < 0 || right < 0 {
                    self.scan_leaf(
                        current,
                        query,
                        partition_key,
                        best_dists,
                        best_labels,
                        best_indices,
                        stats,
                        track_part_keys,
                    );
                } else {
                    let l = left as usize;
                    let r = right as usize;

                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        use std::arch::x86_64::*;
                        _mm_prefetch(
                            self.nodes_base.add(r * layout::NODE_STRIDE) as *const i8,
                            _MM_HINT_T0,
                        );
                    }

                    let lb = lower_bound_box(
                        query,
                        unsafe { layout::node_min(self.nodes_base, l) },
                        unsafe { layout::node_max(self.nodes_base, l) },
                    );
                    let rb = lower_bound_box(
                        query,
                        unsafe { layout::node_min(self.nodes_base, r) },
                        unsafe { layout::node_max(self.nodes_base, r) },
                    );

                    let (near_idx, near_bound, far_idx, far_bound) = if lb <= rb {
                        (l, lb, r, rb)
                    } else {
                        (r, rb, l, lb)
                    };

                    if far_bound <= best_dists[K - 1] && stack_len < TREE_STACK_CAPACITY {
                        stack_nodes[stack_len] = far_idx;
                        stack_bounds[stack_len] = far_bound;
                        stack_len += 1;
                    }

                    if near_bound <= best_dists[K - 1] {
                        current = near_idx;
                        current_bound = near_bound;
                        continue;
                    }
                }
            }

            if stack_len == 0 {
                break;
            }

            stack_len -= 1;
            current = stack_nodes[stack_len];
            current_bound = stack_bounds[stack_len];
        }
    }

    fn scan_leaf(
        &self,
        node_idx: usize,
        query: &QueryVector,
        partition_key: u32,
        best_dists: &mut [i64; K],
        best_labels: &mut [u8; K],
        best_indices: &mut [u32; K],
        stats: &mut Option<&mut SearchStats>,
        track_part_keys: &mut Option<&mut [u32; K]>,
    ) {
        let start_block = unsafe { layout::node_start(self.nodes_base, node_idx) };
        let node_len = unsafe { layout::node_len(self.nodes_base, node_idx) };
        let blocks = (node_len + LANES - 1) / LANES;
        if let Some(s) = stats.as_deref_mut() {
            s.leaves_scanned += 1;
            s.blocks_scanned += blocks as u32;
        }
        let vectors = self.vectors();
        let labels = self.labels();
        let ref_indices = self.ref_indices();
        let q_pairs = unsafe { query_pairs_avx2(query) };
        debug_assert!(
            start_block + blocks <= self.vectors_len / (DIMS * LANES),
            "scan_leaf OOB: start_block={}, blocks={}, total_blocks={}",
            start_block,
            blocks,
            self.vectors_len / (DIMS * LANES)
        );

        for b in 0..blocks {
            let block_idx = start_block + b;
            let block_base = block_idx * DIMS * LANES;

            #[cfg(target_arch = "x86_64")]
            if b + 1 < blocks {
                unsafe {
                    use std::arch::x86_64::*;
                    let next_base = (start_block + b + 1) * DIMS * LANES;
                    let ptr = self.vectors.add(next_base) as *const i8;
                    _mm_prefetch(ptr, _MM_HINT_T0);
                    _mm_prefetch(ptr.add(64), _MM_HINT_T0);
                    _mm_prefetch(ptr.add(128), _MM_HINT_T0);
                    _mm_prefetch(ptr.add(192), _MM_HINT_T0);

                    let labels_ptr = self.labels.add((start_block + b + 1) * LANES) as *const i8;
                    _mm_prefetch(labels_ptr, _MM_HINT_T0);
                }
            }

            let (mask, dists) = unsafe {
                scan_block_pair_avx2_bounded(vectors, block_base, &q_pairs, best_dists[K - 1])
            };
            if mask == 0 {
                continue;
            }
            let labels_base = block_idx * LANES;
            let lane_count = (node_len - b * LANES).min(LANES);
            let mut mask = mask & ((1u32 << lane_count) - 1);
            while mask != 0 {
                let i = mask.trailing_zeros() as usize;
                mask &= mask - 1;
                insert_best(
                    dists[i] as i64,
                    labels[labels_base + i],
                    ref_indices[labels_base + i],
                    partition_key,
                    best_dists,
                    best_labels,
                    best_indices,
                    track_part_keys,
                );
            }
        }
    }

    fn vectors(&self) -> &[i16] {
        unsafe { slice::from_raw_parts(self.vectors, self.vectors_len) }
    }

    fn labels(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.labels, self.labels_len) }
    }

    fn ref_indices(&self) -> &[u32] {
        unsafe { slice::from_raw_parts(self.ref_indices, self.ref_indices_len) }
    }

    #[inline(always)]
    fn node_class_bits(&self, node_idx: usize) -> u8 {
        debug_assert!(node_idx < self.node_class_bits_len);
        unsafe { *self.node_class_bits.add(node_idx) }
    }

    fn advise_hugepages(&self) {
        #[cfg(target_os = "linux")]
        unsafe {
            let vptr = self.vectors as *mut libc::c_void;
            let vlen = self.vectors_len * mem::size_of::<i16>();
            libc::madvise(vptr, vlen, libc::MADV_HUGEPAGE);

            let lptr = self.labels as *mut libc::c_void;
            let llen = self.labels_len;
            libc::madvise(lptr, llen, libc::MADV_HUGEPAGE);

            let rptr = self.ref_indices as *mut libc::c_void;
            let rlen = self.ref_indices_len * mem::size_of::<u32>();
            libc::madvise(rptr, rlen, libc::MADV_HUGEPAGE);
        }
    }
}
