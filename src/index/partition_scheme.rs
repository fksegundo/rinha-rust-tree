use crate::index::build::Reference;
use crate::{DIMS, K, QueryVector};

pub const SCHEME_ID_LEARNED_TREE: i16 = 2;
const LEARNED_SAMPLE_QUERIES: usize = 1024;
pub const LEARNED_TREE_DEFAULT_DEPTH: usize = 8;
pub const LEARNED_TREE_MAX_DEPTH: usize = 10;

#[derive(Clone, Debug)]
pub struct PartitionScheme {
    pub name: String,
    pub tree_depth: usize,
    pub tree_predicates: Vec<TreePredicate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Candidate {
    dim: u8,
    threshold: i16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreePredicate {
    pub dim: u8,
    pub threshold: i16,
    pub enabled: bool,
}

impl PartitionScheme {
    pub fn recommended() -> Self {
        Self::learned_tree("tree256", LEARNED_TREE_DEFAULT_DEPTH)
    }

    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "tree256" => Some(Self::learned_tree("tree256", 8)),
            _ => None,
        }
    }

    pub fn learned_tree(name: &str, tree_depth: usize) -> Self {
        Self {
            name: name.to_string(),
            tree_depth,
            tree_predicates: Vec::new(),
        }
    }

    pub fn from_header(
        scheme_id: i16,
        scheme_param: usize,
        tree_predicates: Vec<TreePredicate>,
    ) -> Result<Self, String> {
        if scheme_id != SCHEME_ID_LEARNED_TREE {
            return Err(format!("Unsupported partition scheme ID: {}", scheme_id));
        }
        Ok(Self {
            name: "tree_loaded".to_string(),
            tree_depth: scheme_param,
            tree_predicates,
        })
    }

    pub fn scheme_id(&self) -> i16 {
        SCHEME_ID_LEARNED_TREE
    }

    pub fn key_bits(&self) -> usize {
        self.tree_depth.min(LEARNED_TREE_MAX_DEPTH)
    }

    pub fn prepare(&mut self, references: &[Reference]) {
        if self.tree_predicates.is_empty() {
            self.tree_predicates = learn_tree_predicates(references, self.tree_depth, &self.name);
        }
    }

    pub fn compute_key(&self, vector: &QueryVector) -> u32 {
        compute_tree_key(vector, self.tree_depth, &self.tree_predicates)
    }
}

#[inline]
pub fn compute_tree_key(
    vector: &QueryVector,
    tree_depth: usize,
    predicates: &[TreePredicate],
) -> u32 {
    let mut key = 0u32;
    let mut node = 0usize;
    for _ in 0..tree_depth.min(LEARNED_TREE_MAX_DEPTH) {
        let side = if let Some(predicate) = predicates.get(node) {
            predicate.enabled && vector[predicate.dim as usize] > predicate.threshold
        } else {
            false
        };
        key = (key << 1) | u32::from(side);
        node = node * 2 + 1 + usize::from(side);
    }
    key
}

fn learn_tree_predicates(
    references: &[Reference],
    tree_depth: usize,
    scheme_name: &str,
) -> Vec<TreePredicate> {
    let tree_depth = tree_depth.min(LEARNED_TREE_MAX_DEPTH);
    if references.len() < K + 1 || tree_depth == 0 {
        return Vec::new();
    }

    let node_count = (1usize << tree_depth) - 1;
    let mut tree = vec![
        TreePredicate {
            dim: 0,
            threshold: 0,
            enabled: false,
        };
        node_count
    ];
    let query_idx = sample_indices(references.len(), LEARNED_SAMPLE_QUERIES);
    let neighbors = exact_topk(references, &query_idx);
    let candidates = candidate_predicates(references);
    let positions: Vec<usize> = (0..query_idx.len()).collect();

    train_tree_node(
        references,
        &query_idx,
        &neighbors,
        &candidates,
        &positions,
        0,
        0,
        tree_depth,
        scheme_name,
        &mut tree,
    );
    tree
}

#[allow(clippy::too_many_arguments)]
fn train_tree_node(
    references: &[Reference],
    query_idx: &[usize],
    neighbors: &[[usize; K]],
    candidates: &[Candidate],
    positions: &[usize],
    node: usize,
    depth: usize,
    max_depth: usize,
    scheme_name: &str,
    tree: &mut [TreePredicate],
) {
    if depth >= max_depth || node >= tree.len() || positions.len() < 8 {
        return;
    }

    let mut best: Option<(f64, Candidate)> = None;
    for &candidate in candidates {
        let mut left = 0usize;
        let mut right = 0usize;
        let mut coloc_sum = 0f64;

        for &pos in positions {
            let qi = query_idx[pos];
            let q_side = predicate_matches(&references[qi].vector, candidate);
            if q_side {
                right += 1;
            } else {
                left += 1;
            }

            let mut same = 0usize;
            for &ni in &neighbors[pos] {
                if predicate_matches(&references[ni].vector, candidate) == q_side {
                    same += 1;
                }
            }
            coloc_sum += same as f64 / K as f64;
        }

        let min_side = left.min(right);
        if min_side < 4 {
            continue;
        }
        let n = positions.len() as f64;
        let coloc = coloc_sum / n;
        let imbalance = (left.max(right) as f64 / n - 0.55).max(0.0);
        let label_sep = label_separation(references, candidate);

        // Use locality profile weights
        let imbalance_weight = 0.35;
        let label_weight = 0.03;
        let score = coloc - imbalance_weight * imbalance + label_weight * label_sep;

        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, candidate));
        }
    }

    let Some((score, predicate)) = best else {
        return;
    };
    if training_logs_enabled() {
        eprintln!(
            "[{}] depth={} node={} dim={} threshold={} queries={} score={:.4}",
            scheme_name,
            depth,
            node,
            predicate.dim,
            predicate.threshold,
            positions.len(),
            score
        );
    }
    tree[node] = TreePredicate {
        dim: predicate.dim,
        threshold: predicate.threshold,
        enabled: true,
    };

    let mut left_positions = Vec::with_capacity(positions.len() / 2);
    let mut right_positions = Vec::with_capacity(positions.len() / 2);
    for &pos in positions {
        let qi = query_idx[pos];
        if predicate_matches(&references[qi].vector, predicate) {
            right_positions.push(pos);
        } else {
            left_positions.push(pos);
        }
    }

    train_tree_node(
        references,
        query_idx,
        neighbors,
        candidates,
        &left_positions,
        node * 2 + 1,
        depth + 1,
        max_depth,
        scheme_name,
        tree,
    );
    train_tree_node(
        references,
        query_idx,
        neighbors,
        candidates,
        &right_positions,
        node * 2 + 2,
        depth + 1,
        max_depth,
        scheme_name,
        tree,
    );
}

fn candidate_predicates(references: &[Reference]) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for dim in 0..DIMS {
        let mut values: Vec<i16> = references.iter().map(|r| r.vector[dim]).collect();
        values.sort_unstable();
        values.dedup();
        if values.len() <= 1 {
            continue;
        }

        if values.len() <= 64 {
            for pair in values.windows(2) {
                candidates.push(Candidate {
                    dim: dim as u8,
                    threshold: midpoint(pair[0], pair[1]),
                });
            }
        } else {
            let n = values.len();
            for q in 1..16 {
                let idx = ((n - 1) * q) / 16;
                candidates.push(Candidate {
                    dim: dim as u8,
                    threshold: values[idx],
                });
            }
        }
    }
    candidates.sort_unstable_by_key(|p| (p.dim, p.threshold));
    candidates.dedup();
    candidates
}

#[inline]
fn midpoint(a: i16, b: i16) -> i16 {
    (((a as i32) + (b as i32)) / 2) as i16
}

#[inline]
fn predicate_matches(vector: &QueryVector, predicate: Candidate) -> bool {
    vector[predicate.dim as usize] > predicate.threshold
}

fn sample_indices(n: usize, max_samples: usize) -> Vec<usize> {
    let sample = n.min(max_samples).max(1);
    let step = (n / sample).max(1);
    (0..n).step_by(step).take(sample).collect()
}

fn exact_topk(references: &[Reference], query_idx: &[usize]) -> Vec<[usize; K]> {
    let threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);
    let chunk = (query_idx.len() + threads - 1) / threads;
    let mut out = vec![[0usize; K]; query_idx.len()];

    std::thread::scope(|scope| {
        for (out_chunk, q_chunk) in out.chunks_mut(chunk).zip(query_idx.chunks(chunk)) {
            scope.spawn(move || {
                for (slot, &qi) in out_chunk.iter_mut().zip(q_chunk.iter()) {
                    *slot = topk_one(references, qi);
                }
            });
        }
    });
    out
}

fn topk_one(references: &[Reference], qi: usize) -> [usize; K] {
    let q = &references[qi].vector;
    let mut best_d = [i64::MAX; K];
    let mut best_i = [usize::MAX; K];
    for (ri, r) in references.iter().enumerate() {
        if ri == qi {
            continue;
        }
        let mut dist = 0i64;
        for d in 0..DIMS {
            let diff = q[d] as i64 - r.vector[d] as i64;
            dist += diff * diff;
        }
        if dist >= best_d[K - 1] {
            continue;
        }
        let mut pos = K - 1;
        while pos > 0 && dist < best_d[pos - 1] {
            best_d[pos] = best_d[pos - 1];
            best_i[pos] = best_i[pos - 1];
            pos -= 1;
        }
        best_d[pos] = dist;
        best_i[pos] = ri;
    }
    best_i
}

fn label_separation(references: &[Reference], predicate: Candidate) -> f64 {
    let mut pos = [0usize; 2];
    let mut total = [0usize; 2];
    for r in references
        .iter()
        .step_by((references.len() / 100_000).max(1))
    {
        let label = r.label as usize;
        total[label] += 1;
        if predicate_matches(&r.vector, predicate) {
            pos[label] += 1;
        }
    }
    let p0 = if total[0] == 0 {
        0.0
    } else {
        pos[0] as f64 / total[0] as f64
    };
    let p1 = if total[1] == 0 {
        0.0
    } else {
        pos[1] as f64 / total[1] as f64
    };
    (p0 - p1).abs()
}

fn training_logs_enabled() -> bool {
    std::env::var("RINHA_PARTITION_TRAIN_LOG").as_deref() == Ok("1")
}
