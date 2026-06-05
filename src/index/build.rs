use crate::index::format::IndexWriter;
use crate::index::partition_scheme::PartitionScheme;
use crate::{DIMS, PACKED_DIMS, QueryVector, SCALE};
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::Read;

pub const LANES: usize = 8;

#[derive(Clone)]
pub struct Reference {
    pub vector: QueryVector,
    pub label: u8,
}

pub fn load_references(path: &str) -> Result<Vec<Reference>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut json_str = String::new();
    if path.ends_with(".gz") {
        let mut decoder = GzDecoder::new(file);
        decoder
            .read_to_string(&mut json_str)
            .map_err(|e| e.to_string())?;
    } else {
        let mut reader = std::io::BufReader::new(file);
        reader
            .read_to_string(&mut json_str)
            .map_err(|e| e.to_string())?;
    }

    let json: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| format!("JSON parse error: {}", e))?;

    let array = json.as_array().ok_or("expected top-level array")?;

    let mut references = Vec::with_capacity(array.len());

    for item in array {
        let vec = item
            .get("vector")
            .and_then(|v| v.as_array())
            .ok_or("missing vector array")?;
        if vec.len() != DIMS {
            return Err(format!("expected {} dims, got {}", DIMS, vec.len()));
        }

        let mut vector = [0i16; PACKED_DIMS];
        for (i, val) in vec.iter().enumerate() {
            let f = val.as_f64().ok_or("non-numeric vector value")?;
            vector[i] = quantize(f);
        }

        let label_str = item
            .get("label")
            .and_then(|v| v.as_str())
            .ok_or("missing label")?;
        let label = if label_str == "fraud" { 1u8 } else { 0u8 };

        references.push(Reference { vector, label });
    }

    Ok(references)
}

#[inline]
fn quantize(value: f64) -> i16 {
    if value <= -1.0 {
        -SCALE
    } else if value <= 0.0 {
        0
    } else if value >= 1.0 {
        SCALE
    } else {
        (value * SCALE as f64).round() as i16
    }
}

struct NodeEntry {
    left: i32,
    right: i32,
    start: usize,
    len: usize,
    min: QueryVector,
    max: QueryVector,
    class_bits: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KdSplitStrategy {
    Widest,
    Variance,
}

pub fn build_index(
    references: Vec<Reference>,
    leaf_size: usize,
    mut scheme: PartitionScheme,
) -> Result<Vec<u8>, String> {
    let leaf_size = leaf_size.clamp(LANES, 2048);
    let split_strategy = kd_split_strategy();

    scheme.prepare(&references);
    eprintln!(
        "[build] scheme={} scheme_id={} tree_depth={} (persisted in header), kd_split={:?}",
        scheme.name,
        scheme.scheme_id(),
        scheme.tree_depth,
        split_strategy
    );

    let mut writer = IndexWriter::new();
    writer.write_header(
        references.len() as i32,
        scheme.scheme_id(),
        scheme.tree_depth as i16,
        &scheme.tree_predicates,
    )?;

    let mut partitions: HashMap<u32, Vec<usize>> = HashMap::new();
    for (idx, ref_item) in references.iter().enumerate() {
        let key = scheme.compute_key(&ref_item.vector);
        partitions.entry(key).or_default().push(idx);
    }

    let mut all_blocks: Vec<(QueryVector, u8, u32)> = Vec::new();
    let mut nodes: Vec<NodeEntry> = Vec::new();
    let mut partition_meta: Vec<(u32, usize)> = Vec::new();

    let mut sorted_keys: Vec<u32> = partitions.keys().copied().collect();
    sorted_keys.sort_unstable();

    for key in &sorted_keys {
        let indices = &partitions[key];
        let root = build_node(
            &references,
            indices,
            leaf_size,
            split_strategy,
            &mut all_blocks,
            &mut nodes,
        );
        partition_meta.push((*key, root));
    }

    let partition_count = partition_meta.len() as i32;
    writer.write_partition_count(partition_count)?;
    let node_count = nodes.len() as i32;
    writer.write_node_count(node_count)?;

    for (key, root) in &partition_meta {
        let root_node = &nodes[*root];
        writer.write_partition_entry(*key, *root, root_node.len, root_node.min, root_node.max)?;
    }

    for node in &nodes {
        let block_start = node.start / LANES;
        writer.write_node_entry(
            node.left,
            node.right,
            block_start,
            node.len,
            node.min,
            node.max,
        )?;
    }

    let total_blocks = all_blocks.len() / LANES;
    writer.write_block_count(total_blocks as i32)?;

    for b in 0..total_blocks {
        for pair in 0..(DIMS / 2) {
            for l in 0..LANES {
                let (vec, _, _) = all_blocks[b * LANES + l];
                writer.write_i16(vec[pair * 2])?;
                writer.write_i16(vec[pair * 2 + 1])?;
            }
        }
    }

    for b in 0..total_blocks {
        for l in 0..LANES {
            let (_, label, _) = all_blocks[b * LANES + l];
            writer.write_u8(label)?;
        }
    }

    writer.align_to(std::mem::align_of::<u32>());
    for b in 0..total_blocks {
        for l in 0..LANES {
            let (_, _, ref_index) = all_blocks[b * LANES + l];
            writer.write_u32(ref_index)?;
        }
    }

    for node in &nodes {
        writer.write_u8(node.class_bits)?;
    }

    Ok(writer.into_bytes())
}

fn build_node(
    references: &[Reference],
    indices: &[usize],
    leaf_size: usize,
    split_strategy: KdSplitStrategy,
    all_blocks: &mut Vec<(QueryVector, u8, u32)>,
    nodes: &mut Vec<NodeEntry>,
) -> usize {
    let mut min = [i16::MAX; PACKED_DIMS];
    let mut max = [i16::MIN; PACKED_DIMS];
    for &idx in indices {
        let ref_item = &references[idx];
        for d in 0..PACKED_DIMS {
            min[d] = min[d].min(ref_item.vector[d]);
            max[d] = max[d].max(ref_item.vector[d]);
        }
    }

    let node_idx = nodes.len();
    nodes.push(NodeEntry {
        left: -1,
        right: -1,
        start: 0,
        len: 0,
        min,
        max,
        class_bits: 0,
    });

    if indices.len() <= leaf_size {
        let leaf_start = all_blocks.len();
        let blocks = (indices.len() + LANES - 1) / LANES;
        let mut class_bits = 0u8;

        for b in 0..blocks {
            for l in 0..LANES {
                let i = b * LANES + l;
                if i < indices.len() {
                    let ref_idx = indices[i];
                    let ref_item = &references[ref_idx];
                    class_bits |= 1u8 << ref_item.label.min(7);
                    all_blocks.push((ref_item.vector, ref_item.label, ref_idx as u32));
                } else {
                    all_blocks.push(([0i16; PACKED_DIMS], 0u8, u32::MAX));
                }
            }
        }

        nodes[node_idx] = NodeEntry {
            left: -1,
            right: -1,
            start: leaf_start,
            len: indices.len(),
            min,
            max,
            class_bits,
        };
        return node_idx;
    }

    let split_dim = match split_strategy {
        KdSplitStrategy::Widest => widest_dimension(&min, &max),
        KdSplitStrategy::Variance => variance_dimension(references, indices, &min, &max),
    };
    let mut sorted = indices.to_vec();
    sorted.sort_unstable_by(|&a, &b| {
        references[a].vector[split_dim].cmp(&references[b].vector[split_dim])
    });

    let left_len = sorted.len() / 2;
    let (left_indices, right_indices) = sorted.split_at(left_len);

    let left_node = build_node(
        references,
        left_indices,
        leaf_size,
        split_strategy,
        all_blocks,
        nodes,
    );
    let right_node = build_node(
        references,
        right_indices,
        leaf_size,
        split_strategy,
        all_blocks,
        nodes,
    );

    let left_info = &nodes[left_node];
    let right_info = &nodes[right_node];

    nodes[node_idx] = NodeEntry {
        left: left_node as i32,
        right: right_node as i32,
        start: left_info.start,
        len: left_info.len + right_info.len,
        min,
        max,
        class_bits: left_info.class_bits | right_info.class_bits,
    };

    node_idx
}

fn widest_dimension(min: &QueryVector, max: &QueryVector) -> usize {
    let mut best_dim = 0usize;
    let mut best_width = i16::MIN;
    for d in 0..DIMS {
        let width = max[d] - min[d];
        if width > best_width {
            best_width = width;
            best_dim = d;
        }
    }
    best_dim
}

fn variance_dimension(
    references: &[Reference],
    indices: &[usize],
    min: &QueryVector,
    max: &QueryVector,
) -> usize {
    let n = indices.len() as i128;
    let mut best_dim = widest_dimension(min, max);
    let mut best_score = i128::MIN;

    for d in 0..DIMS {
        if min[d] == max[d] {
            continue;
        }

        let mut sum = 0i128;
        let mut sum_sq = 0i128;
        for &idx in indices {
            let v = references[idx].vector[d] as i128;
            sum += v;
            sum_sq += v * v;
        }

        let score = n * sum_sq - sum * sum;
        if score > best_score {
            best_score = score;
            best_dim = d;
        }
    }

    best_dim
}

fn kd_split_strategy() -> KdSplitStrategy {
    match std::env::var("RINHA_KD_SPLIT_STRATEGY").ok().as_deref() {
        Some("variance") => KdSplitStrategy::Variance,
        _ => KdSplitStrategy::Widest,
    }
}
