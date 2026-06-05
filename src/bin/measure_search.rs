//! Micro-benchmark of the pure exact k-NN search.
//!
//! Reports per-query search compute time and work counters (blocks_scanned, partitions_visited).
//! Isolates leaf-scan/partition overhead from HTTP/epoll.
//!
//! Usages:
//!   measure_search <index.idx> <test.json> [repeats]
//!   measure_search compare --refs <refs.json.gz> --queries <test.json> [--leaf-size 56] [--limit 10000] [--split-strategy widest|variance|both]

use rinha_rust_tree::index::SpecialistIndex;
use rinha_rust_tree::index::partition_scheme::PartitionScheme;
use rinha_rust_tree::vector;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "compare" {
        run_compare(&args);
    } else {
        run_single(&args);
    }
}

fn run_single(args: &[String]) {
    if args.len() < 3 {
        eprintln!("Usage: {} <index.idx> <test.json> [repeats]", args[0]);
        std::process::exit(1);
    }
    let index_path = &args[1];
    let json_path = &args[2];
    let repeats: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);

    let index =
        SpecialistIndex::open(index_path).unwrap_or_else(|e| panic!("failed to open index: {}", e));

    let json_str = std::fs::read_to_string(json_path).expect("read test json");
    let root: serde_json::Value = serde_json::from_str(&json_str).expect("parse json");
    let entries = root
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("entries array");

    let mut queries: Vec<[i16; 16]> = Vec::with_capacity(entries.len());
    let mut expected_counts: Vec<Option<u8>> = Vec::with_capacity(entries.len());
    let mut expected_approved: Vec<Option<bool>> = Vec::with_capacity(entries.len());
    for entry in entries {
        let request = entry.get("request").expect("entry.request");
        let body = serde_json::to_vec(request).expect("serialize request");
        let mut q = [0i16; 16];
        if vector::parse_query(&body, &mut q).is_ok() {
            queries.push(q);
            expected_counts.push(expected_count(entry));
            expected_approved.push(entry.get("expected_approved").and_then(|v| v.as_bool()));
        }
    }
    eprintln!("parsed {} queries from {}", queries.len(), json_path);

    let mut blocks: Vec<u32> = Vec::with_capacity(queries.len());
    let mut leaves: Vec<u32> = Vec::with_capacity(queries.len());
    let mut parts: Vec<u32> = Vec::with_capacity(queries.len());
    let mut secondaries: Vec<u32> = Vec::with_capacity(queries.len());
    let mut score_matches = 0usize;
    let mut score_total = 0usize;
    let mut approved_matches = 0usize;
    let mut approved_total = 0usize;
    let mut checksum: u64 = 0;
    for (i, q) in queries.iter().enumerate() {
        let (c, s) = index.predict_fraud_count_with_stats(q);
        checksum = checksum.wrapping_add(c as u64);
        blocks.push(s.blocks_scanned);
        leaves.push(s.leaves_scanned);
        parts.push(s.partitions_visited);
        secondaries.push(s.secondary_partitions);

        if let Some(expected) = expected_counts[i] {
            score_total += 1;
            if c == expected {
                score_matches += 1;
            }
        }
        if let Some(expected) = expected_approved[i] {
            approved_total += 1;
            if (c < 3) == expected {
                approved_matches += 1;
            }
        }
    }

    let mut ns: Vec<u64> = Vec::with_capacity(queries.len() * repeats);
    for _ in 0..repeats {
        for q in &queries {
            let t0 = Instant::now();
            let c = index.predict_fraud_count(q);
            ns.push(t0.elapsed().as_nanos() as u64);
            checksum = checksum.wrapping_add(c as u64);
        }
    }
    std::hint::black_box(checksum);

    println!("index: {}", index_path);
    report_u64("compute_ns", &mut ns);
    report_u32("blocks_scanned", &mut blocks);
    report_u32("leaves_scanned", &mut leaves);
    report_u32("partitions_visited", &mut parts);
    report_u32("secondary_partitions", &mut secondaries);
    if score_total > 0 {
        println!(
            "  score_accuracy      {}/{} ({:.4}%)",
            score_matches,
            score_total,
            score_matches as f64 * 100.0 / score_total as f64
        );
    }
    if approved_total > 0 {
        println!(
            "  approved_accuracy   {}/{} ({:.4}%)",
            approved_matches,
            approved_total,
            approved_matches as f64 * 100.0 / approved_total as f64
        );
    }
}

fn expected_count(entry: &serde_json::Value) -> Option<u8> {
    let score = entry.get("expected_fraud_score")?.as_f64()?;
    let count = (score * 5.0).round();
    if (0.0..=5.0).contains(&count) {
        Some(count as u8)
    } else {
        None
    }
}

fn run_compare(args: &[String]) {
    let mut refs_path: Option<String> = None;
    let mut queries_path: Option<String> = None;
    let mut leaf_size = 56usize;
    let mut limit = 10000usize;
    let mut split_strategy = "widest".to_string();

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--refs" if i + 1 < args.len() => {
                refs_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--queries" if i + 1 < args.len() => {
                queries_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--leaf-size" if i + 1 < args.len() => {
                leaf_size = args[i + 1].parse().unwrap_or(56);
                i += 2;
            }
            "--limit" if i + 1 < args.len() => {
                limit = args[i + 1].parse().unwrap_or(10000);
                i += 2;
            }
            "--split-strategy" if i + 1 < args.len() => {
                split_strategy = args[i + 1].clone();
                i += 2;
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    let refs_path = refs_path.expect("Missing required --refs <path>");
    let queries_path = queries_path.expect("Missing required --queries <path>");

    eprintln!(
        "[{}] Loading references from {}...",
        format_time(),
        refs_path
    );
    let references = rinha_rust_tree::index::build::load_references(&refs_path)
        .unwrap_or_else(|e| panic!("failed to load references: {}", e));
    eprintln!("[{}] Loaded {} references", format_time(), references.len());

    eprintln!(
        "[{}] Parsing queries from {}...",
        format_time(),
        queries_path
    );
    let json_str = std::fs::read_to_string(&queries_path).expect("read test json");
    let root: serde_json::Value = serde_json::from_str(&json_str).expect("parse json");
    let entries = root
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("entries array");

    let mut queries: Vec<[i16; 16]> = Vec::with_capacity(entries.len().min(limit));
    for entry in entries {
        if queries.len() >= limit {
            break;
        }
        let request = entry.get("request").expect("entry.request");
        let body = serde_json::to_vec(request).expect("serialize request");
        let mut q = [0i16; 16];
        if vector::parse_query(&body, &mut q).is_ok() {
            queries.push(q);
        }
    }
    eprintln!(
        "[{}] Parsed {} queries (limit={})",
        format_time(),
        queries.len(),
        limit
    );

    println!(
        "\n=== Scheme Comparison Matrix (leaf_size={}) ===",
        leaf_size
    );
    println!(
        "{:<14} | {:<12} | {:<12} | {:<12} | {:<18} | {:<10}",
        "Scheme", "p50 (µs)", "p99 (µs)", "LeafBlocks", "SecondaryParts", "WorkScore"
    );
    println!("{}", "-".repeat(89));

    let temp_idx_path = format!("/tmp/rinha-measure-search-{}.idx", std::process::id());
    let schemes = ["tree256"];
    let split_strategies: &[&str] = match split_strategy.as_str() {
        "both" => &["widest", "variance"],
        "variance" => &["variance"],
        _ => &["widest"],
    };

    let mut ranked = Vec::new();

    for &split in split_strategies {
        unsafe {
            std::env::set_var("RINHA_KD_SPLIT_STRATEGY", split);
        }
        for &scheme_name in &schemes {
            let scheme = PartitionScheme::by_name(scheme_name).unwrap();
            let label = format!("{}:{}", scheme_name, split);
            eprintln!(
                "[{}] Building in-memory index for {}...",
                format_time(),
                label
            );

            let index_bytes = rinha_rust_tree::index::build::build_index(
                references.clone(),
                leaf_size,
                scheme.clone(),
            )
            .unwrap();

            std::fs::write(&temp_idx_path, &index_bytes).expect("write temp index");

            let index = SpecialistIndex::open(&temp_idx_path).unwrap();

            // Measure stats & timings
            let mut total_blocks = 0u64;
            let mut total_secondaries = 0u64;
            let mut timings = Vec::with_capacity(queries.len());

            for q in &queries {
                let t0 = Instant::now();
                let (_, s) = index.predict_fraud_count_with_stats(q);
                timings.push(t0.elapsed().as_nanos() as u64);
                total_blocks += s.blocks_scanned as u64;
                total_secondaries += s.secondary_partitions as u64;
            }

            timings.sort_unstable();
            let q_count = queries.len() as f64;
            let p50 = timings[(0.50 * timings.len() as f64) as usize] as f64 / 1000.0;
            let p99 = timings[(0.99 * timings.len() as f64) as usize] as f64 / 1000.0;

            let avg_blocks = total_blocks as f64 / q_count;
            let avg_secondaries = total_secondaries as f64 / q_count;
            let work_score = avg_blocks + 100.0 * avg_secondaries;

            println!(
                "{:<14} | {:<12.1} | {:<12.1} | {:<12.1} | {:<18.2} | {:<10.1}",
                label, p50, p99, avg_blocks, avg_secondaries, work_score
            );

            ranked.push((work_score, label, p50, p99));
        }
    }

    let _ = std::fs::remove_file(temp_idx_path);

    ranked.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!("\n=== Ranks (by WorkScore: lower is better) ===");
    for (i, (score, name, p50, p99)) in ranked.iter().enumerate() {
        println!(
            "  {}. {} (WorkScore={:.1}, p50={:.1}µs, p99={:.1}µs)",
            i + 1,
            name,
            score,
            p50,
            p99
        );
    }
}

fn format_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let hour = (now / 3600) % 24;
    let minute = (now / 60) % 60;
    let second = now % 60;
    format!("{:02}:{:02}:{:02}", hour, minute, second)
}

fn report_u64(name: &str, v: &mut [u64]) {
    v.sort_unstable();
    let n = v.len();
    let pct = |p: f64| v[(((p / 100.0) * n as f64) as usize).min(n - 1)];
    let mean = v.iter().sum::<u64>() as f64 / n as f64;
    println!(
        "  {:<20} n={} mean={:.0} p50={} p90={} p99={} p999={} max={}",
        name,
        n,
        mean,
        pct(50.0),
        pct(90.0),
        pct(99.0),
        pct(99.9),
        v[n - 1]
    );
}

fn report_u32(name: &str, v: &mut [u32]) {
    v.sort_unstable();
    let n = v.len();
    let pct = |p: f64| v[(((p / 100.0) * n as f64) as usize).min(n - 1)];
    let mean = v.iter().map(|&x| x as u64).sum::<u64>() as f64 / n as f64;
    println!(
        "  {:<20} n={} mean={:.1} p50={} p90={} p99={} max={}",
        name,
        n,
        mean,
        pct(50.0),
        pct(90.0),
        pct(99.0),
        v[n - 1]
    );
}
