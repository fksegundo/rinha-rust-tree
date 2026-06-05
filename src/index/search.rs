use crate::{DIMS, K, QueryVector};

const LANES: usize = 8;
const DIM_PAIRS: usize = DIMS / 2;
const DEFER_STACK_CAPACITY: usize = 4096;

pub struct PendingSubtrees {
    pub enabled: bool,
    pub label: Option<u8>,
    pub roots: [usize; DEFER_STACK_CAPACITY],
    pub bounds: [i64; DEFER_STACK_CAPACITY],
    pub len: usize,
}

impl PendingSubtrees {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            label: None,
            roots: [0; DEFER_STACK_CAPACITY],
            bounds: [0; DEFER_STACK_CAPACITY],
            len: 0,
        }
    }

    #[inline(always)]
    pub fn try_defer(
        &mut self,
        node_idx: usize,
        bound: i64,
        best_dists: &[i64; K],
        best_labels: &[u8; K],
        node_class_bits: u8,
    ) -> bool {
        if !self.enabled || self.len >= DEFER_STACK_CAPACITY {
            return false;
        }
        let Some(label) = consensus_label(best_dists, best_labels) else {
            return false;
        };
        let needed = 1u8 << (1 - label);
        if node_class_bits == 0 || (node_class_bits & needed) != 0 {
            return false;
        }
        self.label.get_or_insert(label);
        if self.label != Some(label) {
            return false;
        }
        self.roots[self.len] = node_idx;
        self.bounds[self.len] = bound;
        self.len += 1;
        true
    }

    #[inline(always)]
    pub fn should_replay(&self, best_dists: &[i64; K], best_labels: &[u8; K]) -> bool {
        self.len > 0 && consensus_label(best_dists, best_labels) != self.label
    }

    #[inline(always)]
    pub fn pop(&mut self) -> Option<(usize, i64)> {
        if self.len == 0 {
            self.label = None;
            return None;
        }
        self.len -= 1;
        Some((self.roots[self.len], self.bounds[self.len]))
    }
}

#[inline(always)]
pub fn consensus_label(best_dists: &[i64; K], best_labels: &[u8; K]) -> Option<u8> {
    if best_dists[K - 1] == i64::MAX {
        return None;
    }
    let sum = best_labels.iter().map(|&label| label as u32).sum::<u32>();
    if sum == 0 {
        Some(0)
    } else if sum == K as u32 {
        Some(1)
    } else {
        None
    }
}

pub fn replay_pending_if_needed<F>(
    pending_subtrees: &mut PendingSubtrees,
    best_dists: &mut [i64; K],
    best_labels: &mut [u8; K],
    mut search_fn: F,
) where
    F: FnMut(usize, i64, &mut [i64; K], &mut [u8; K]),
{
    if !pending_subtrees.should_replay(best_dists, best_labels) {
        return;
    }
    while let Some((root, bound)) = pending_subtrees.pop() {
        if bound > best_dists[K - 1] {
            continue;
        }
        search_fn(root, bound, best_dists, best_labels);
    }
}

#[inline(always)]
pub fn sort_partition_entries(entries: &mut [(i64, usize)]) {
    let n = entries.len();
    if n <= 1 {
        return;
    }
    if n <= 32 {
        for i in 1..n {
            let mut j = i;
            while j > 0 && entries[j - 1].0 > entries[j].0 {
                entries.swap(j - 1, j);
                j -= 1;
            }
        }
        return;
    }
    entries.sort_unstable_by_key(|&(bound, _)| bound);
}

#[inline(always)]
pub fn insert_best(
    dist: i64,
    label: u8,
    ref_index: u32,
    partition_key: u32,
    best_dists: &mut [i64; K],
    best_labels: &mut [u8; K],
    best_indices: &mut [u32; K],
    track_part_keys: &mut Option<&mut [u32; K]>,
) {
    if !candidate_before(dist, ref_index, best_dists[K - 1], best_indices[K - 1]) {
        return;
    }
    let mut pos = K - 1;
    while pos > 0 && candidate_before(dist, ref_index, best_dists[pos - 1], best_indices[pos - 1]) {
        best_dists[pos] = best_dists[pos - 1];
        best_labels[pos] = best_labels[pos - 1];
        best_indices[pos] = best_indices[pos - 1];
        if let Some(keys) = track_part_keys.as_deref_mut() {
            keys[pos] = keys[pos - 1];
        }
        pos -= 1;
    }
    best_dists[pos] = dist;
    best_labels[pos] = label;
    best_indices[pos] = ref_index;
    if let Some(keys) = track_part_keys.as_deref_mut() {
        keys[pos] = partition_key;
    }
}

#[inline(always)]
pub fn insert_best_fast(
    dist: i64,
    label: u8,
    ref_index: u32,
    best_dists: &mut [i64; K],
    best_labels: &mut [u8; K],
    best_indices: &mut [u32; K],
) {
    if !candidate_before(dist, ref_index, best_dists[K - 1], best_indices[K - 1]) {
        return;
    }
    let mut pos = K - 1;
    while pos > 0 && candidate_before(dist, ref_index, best_dists[pos - 1], best_indices[pos - 1]) {
        best_dists[pos] = best_dists[pos - 1];
        best_labels[pos] = best_labels[pos - 1];
        best_indices[pos] = best_indices[pos - 1];
        pos -= 1;
    }
    best_dists[pos] = dist;
    best_labels[pos] = label;
    best_indices[pos] = ref_index;
}

#[inline(always)]
pub fn candidate_before(dist: i64, ref_index: u32, other_dist: i64, other_index: u32) -> bool {
    dist < other_dist || (dist == other_dist && ref_index < other_index)
}

#[target_feature(enable = "avx2")]
pub unsafe fn query_pairs_avx2(query: &QueryVector) -> [std::arch::x86_64::__m256i; DIM_PAIRS] {
    use std::arch::x86_64::*;
    let mut q_pairs = [_mm256_setzero_si256(); DIM_PAIRS];
    for pair in 0..DIM_PAIRS {
        let lo = query[pair * 2] as u16 as u32;
        let hi = query[pair * 2 + 1] as u16 as u32;
        q_pairs[pair] = _mm256_set1_epi32((lo | (hi << 16)) as i32);
    }
    q_pairs
}

#[target_feature(enable = "avx2")]
pub unsafe fn scan_block_pair_avx2_bounded(
    vectors: &[i16],
    block_base: usize,
    q_pairs: &[std::arch::x86_64::__m256i; DIM_PAIRS],
    limit: i64,
) -> (u32, [i32; LANES]) {
    use std::arch::x86_64::*;
    unsafe {
        let base = vectors.as_ptr().add(block_base);
        let mut acc = _mm256_setzero_si256();
        for pair in 0..DIM_PAIRS {
            let packed = _mm256_loadu_si256(base.add(pair * LANES * 2) as *const __m256i);
            let diff = _mm256_sub_epi16(q_pairs[pair], packed);
            acc = _mm256_add_epi32(acc, _mm256_madd_epi16(diff, diff));
        }

        let mut block_dists = [0i32; LANES];
        if limit < i32::MAX as i64 {
            let below = _mm256_cmpgt_epi32(_mm256_set1_epi32(limit as i32 + 1), acc);
            let mask = _mm256_movemask_ps(_mm256_castsi256_ps(below)) as u32;
            if mask == 0 {
                return (0, block_dists);
            }
            _mm256_storeu_si256(block_dists.as_mut_ptr() as *mut __m256i, acc);
            (mask, block_dists)
        } else {
            _mm256_storeu_si256(block_dists.as_mut_ptr() as *mut __m256i, acc);
            (0xff, block_dists)
        }
    }
}

#[inline(always)]
pub fn lower_bound_box(query: &QueryVector, min: &QueryVector, max: &QueryVector) -> i64 {
    unsafe { lower_bound_box_avx2(query, min, max) }
}

#[target_feature(enable = "avx2")]
pub unsafe fn lower_bound_box_avx2(
    query: &QueryVector,
    min: &QueryVector,
    max: &QueryVector,
) -> i64 {
    use std::arch::x86_64::*;
    unsafe {
        let q = _mm256_loadu_si256(query.as_ptr() as *const __m256i);
        let mn = _mm256_loadu_si256(min.as_ptr() as *const __m256i);
        let mx = _mm256_loadu_si256(max.as_ptr() as *const __m256i);

        let zero = _mm256_setzero_si256();
        let below = _mm256_max_epi16(_mm256_sub_epi16(mn, q), zero);
        let above = _mm256_max_epi16(_mm256_sub_epi16(q, mx), zero);
        let diff = _mm256_max_epi16(below, above);

        let sq = _mm256_madd_epi16(diff, diff);

        let lo = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(sq));
        let hi = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(sq, 1));
        let sum64 = _mm256_add_epi64(lo, hi);

        let sum_hi = _mm256_extracti128_si256(sum64, 1);
        let sum_128 = _mm_add_epi64(_mm256_castsi256_si128(sum64), sum_hi);

        let s0 = _mm_extract_epi64(sum_128, 0);
        let s1 = _mm_extract_epi64(sum_128, 1);

        s0 + s1
    }
}

pub fn read_i32(bytes: &[u8], cursor: &mut usize) -> Result<i32, String> {
    if *cursor + 4 > bytes.len() {
        return Err("unexpected EOF (i32)".to_string());
    }
    let v = i32::from_le_bytes(bytes[*cursor..*cursor + 4].try_into().unwrap());
    *cursor += 4;
    Ok(v)
}

pub fn read_i16(bytes: &[u8], cursor: &mut usize) -> Result<i16, String> {
    if *cursor + 2 > bytes.len() {
        return Err("unexpected EOF (i16)".to_string());
    }
    let v = i16::from_le_bytes(bytes[*cursor..*cursor + 2].try_into().unwrap());
    *cursor += 2;
    Ok(v)
}

pub fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, String> {
    if *cursor + 1 > bytes.len() {
        return Err("unexpected EOF (u8)".to_string());
    }
    let v = bytes[*cursor];
    *cursor += 1;
    Ok(v)
}

pub fn align_cursor(cursor: usize, align: usize) -> usize {
    cursor + ((align - (cursor % align)) % align)
}
