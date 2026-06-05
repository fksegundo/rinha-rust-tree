//! Static partition-locality analysis (no index rebuild / no benchmark).
//!
//! For each candidate `bucket_dim` (the feature used for the equi-freq bits 5-7 of
//! the partition key), reports:
//!   - octile cut points
//!   - partition fan-out (distinct keys) and size balance
//!   - neighbor co-location: of each query's exact top-5 NN, how many share the
//!     query's partition key (Modo A/B benefit) and how many distinct partitions
//!     the top-5 spans (lower = cheaper Modo B).
//!
//! The exact top-5 neighbors are computed once per sampled query (independent of
//! the bucket dim), then every candidate dim is scored against the same neighbors.
//!
//! Usage: analyze_partitions <references.json> [sample_queries]

use rinha_rust_tree::index::build::load_references;
use rinha_rust_tree::{DIMS, K, QueryVector};
use std::time::Instant;

const CANDIDATE_DIMS: &[usize] = &[0, 1, 2, 3, 4, 7, 8, 12, 13];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "resources/references.json".to_string());
    let sample: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1500);

    eprintln!("loading references from {path}...");
    let refs = load_references(&path).expect("load references");
    let n = refs.len();
    eprintln!("loaded {n} references");

    let vectors: Vec<QueryVector> = refs.iter().map(|r| r.vector).collect();

    // Per-dim variance (distance contribution) over all refs.
    let mut var = [0f64; DIMS];
    for d in 0..DIMS {
        let mut sum = 0f64;
        let mut sumsq = 0f64;
        for v in &vectors {
            let x = v[d] as f64;
            sum += x;
            sumsq += x * x;
        }
        let mean = sum / n as f64;
        var[d] = sumsq / n as f64 - mean * mean;
    }

    // Sample queries evenly across the file.
    let step = (n / sample).max(1);
    let query_idx: Vec<usize> = (0..n).step_by(step).collect();
    eprintln!(
        "computing exact top-{K} for {} sampled queries (brute force {}x{})...",
        query_idx.len(),
        query_idx.len(),
        n
    );

    let t0 = Instant::now();
    let neighbors = exact_topk(&vectors, &query_idx);
    eprintln!("brute force done in {:.1}s", t0.elapsed().as_secs_f64());

    println!("\n=== per-dim variance (scaled i16^2, higher = dominates distance) ===");
    let mut order: Vec<usize> = (0..DIMS).collect();
    order.sort_by(|&a, &b| var[b].partial_cmp(&var[a]).unwrap());
    for d in order {
        let tag = match d {
            9 | 10 | 11 => " [binary bit]",
            8 => " [bit3 source]",
            2 => " [bit4 source]",
            5 | 6 => " [-1 sentinel]",
            _ => "",
        };
        println!("  dim {:>2}: var={:>14.1}{}", d, var[d], tag);
    }

    println!("\n=== partition locality by bucket_dim ===");
    println!(
        "{:>4}  {:>6}  {:>10}  {:>8}  {:>10}  {:>12}  {:>12}",
        "dim", "parts", "max_part", "p99", "coloc_top5", "parts_span", "cuts"
    );

    let mut best: Option<(usize, f64)> = None;
    for &d in CANDIDATE_DIMS {
        let cuts = equifreq_cuts(&vectors, d);
        let keys: Vec<u32> = vectors.iter().map(|v| partition_key(v, d, &cuts)).collect();

        // Fan-out + balance.
        let mut counts = std::collections::HashMap::<u32, u32>::new();
        for &k in &keys {
            *counts.entry(k).or_insert(0) += 1;
        }
        let mut sizes: Vec<u32> = counts.values().copied().collect();
        sizes.sort_unstable();
        let parts = sizes.len();
        let max_part = *sizes.last().unwrap_or(&0);
        let p99 = sizes[((sizes.len() as f64 * 0.99) as usize).min(sizes.len().saturating_sub(1))];

        // Co-location of true top-5 neighbors.
        let mut coloc_sum = 0f64;
        let mut span_sum = 0f64;
        for (qi, nbrs) in query_idx.iter().zip(neighbors.iter()) {
            let qk = keys[*qi];
            let mut same = 0u32;
            let mut spankeys = [u32::MAX; K];
            let mut spann = 0usize;
            for &ni in nbrs {
                let nk = keys[ni];
                if nk == qk {
                    same += 1;
                }
                if !spankeys[..spann].contains(&nk) {
                    spankeys[spann] = nk;
                    spann += 1;
                }
            }
            coloc_sum += same as f64 / K as f64;
            span_sum += spann as f64;
        }
        let coloc = coloc_sum / query_idx.len() as f64;
        let span = span_sum / query_idx.len() as f64;

        println!(
            "{:>4}  {:>6}  {:>10}  {:>8}  {:>9.1}%  {:>12.2}  {:?}",
            d,
            parts,
            max_part,
            p99,
            coloc * 100.0,
            span,
            cuts
        );

        // Score: high co-location, low span.
        let score = coloc - 0.1 * span;
        if best.as_ref().map(|(_, s)| score > *s).unwrap_or(true) {
            best = Some((d, score));
        }
    }

    if let Some((d, _)) = best {
        println!("\nBest bucket_dim by (coloc - 0.1*span): {d}  -> RINHA_PARTITION_BUCKET_DIM={d}");
    }
}

/// Octile cut points on `dim` over all references (matches build::compute_equifreq_cuts).
fn equifreq_cuts(vectors: &[QueryVector], dim: usize) -> [i16; 7] {
    if vectors.len() < 8 {
        return [0; 7];
    }
    let mut values: Vec<i16> = vectors.iter().map(|v| v[dim]).collect();
    values.sort_unstable();
    let n = values.len();
    let mut cuts = [0i16; 7];
    for (i, slot) in cuts.iter_mut().enumerate() {
        *slot = values[(n * (i + 1)) / 8];
    }
    cuts
}

/// Partition key with fixed boolean bits + equi-freq bucket on `dim`.
#[inline]
fn partition_key(v: &QueryVector, dim: usize, cuts: &[i16; 7]) -> u32 {
    let mut key = 0u32;
    if v[9] > 0 {
        key |= 1 << 0;
    }
    if v[10] > 0 {
        key |= 1 << 1;
    }
    if v[11] > 0 {
        key |= 1 << 2;
    }
    if v[8] > 2048 {
        key |= 1 << 3;
    }
    if v[2] > 4096 {
        key |= 1 << 4;
    }
    key |= bucket8(v[dim], cuts) << 5;
    key
}

#[inline]
fn bucket8(value: i16, cuts: &[i16; 7]) -> u32 {
    if value <= 0 {
        return 0;
    }
    let mut bucket = 0u32;
    for &c in cuts {
        bucket += (value > c) as u32;
    }
    bucket
}

/// Exact top-K nearest neighbor indices for each query (parallel, brute force).
fn exact_topk(vectors: &[QueryVector], query_idx: &[usize]) -> Vec<[usize; K]> {
    let threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);
    let chunk = (query_idx.len() + threads - 1) / threads;
    let mut out = vec![[0usize; K]; query_idx.len()];

    std::thread::scope(|scope| {
        for (out_chunk, q_chunk) in out.chunks_mut(chunk).zip(query_idx.chunks(chunk)) {
            scope.spawn(move || {
                for (slot, &qi) in out_chunk.iter_mut().zip(q_chunk.iter()) {
                    *slot = topk_one(vectors, qi);
                }
            });
        }
    });
    out
}

#[inline]
fn topk_one(vectors: &[QueryVector], qi: usize) -> [usize; K] {
    let q = &vectors[qi];
    let mut best_d = [i64::MAX; K];
    let mut best_i = [usize::MAX; K];
    for (ri, r) in vectors.iter().enumerate() {
        if ri == qi {
            continue;
        }
        let mut dist = 0i64;
        for d in 0..DIMS {
            let diff = q[d] as i64 - r[d] as i64;
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
