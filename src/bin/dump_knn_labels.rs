//! Label training rows with exact k-NN count + contributing partition keys.
//!
//! Usage:
//!   dump_knn_labels <index.idx> <out.csv> [max_per_file] <test.json>...

use rinha_rust_tree::index::SpecialistIndex;
use rinha_rust_tree::vector;
use rinha_rust_tree::{DIMS, QueryVector};
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "Usage: {} <index.idx> <out.csv> [max_per_file] <test.json>...",
            args[0]
        );
        std::process::exit(1);
    }
    let index_path = &args[1];
    let out_path = &args[2];
    let (max_per_file, json_paths): (usize, Vec<&String>) = match args[3].parse::<usize>() {
        Ok(n) => (n, args[4..].iter().collect()),
        Err(_) => (usize::MAX, args[3..].iter().collect()),
    };
    if json_paths.is_empty() {
        eprintln!("At least one <test.json> path is required");
        std::process::exit(1);
    }

    eprintln!("Opening index {}...", index_path);
    let index =
        SpecialistIndex::open(index_path).unwrap_or_else(|e| panic!("failed to open index: {}", e));

    let mut out = std::fs::File::create(out_path).expect("create csv");
    writeln!(
        out,
        "f0,f1,f2,f3,f4,f5,f6,f7,f8,f9,f10,f11,f12,f13,count,partitions"
    )
    .expect("header");

    let mut query = [0i16; 16];
    let mut total_ok = 0usize;
    let mut total_err = 0usize;

    for path in json_paths {
        eprintln!("Loading {}...", path);
        let json_str = std::fs::read_to_string(path).expect("read test json");
        let root: serde_json::Value = serde_json::from_str(&json_str).expect("parse json");
        let entries = root
            .get("entries")
            .and_then(|v| v.as_array())
            .expect("entries array");

        let n = entries.len().min(max_per_file);
        eprintln!(
            "Labeling {} / {} entries from {}...",
            n,
            entries.len(),
            path
        );

        for (i, entry) in entries.iter().take(n).enumerate() {
            let request = entry.get("request").expect("entry.request");
            let body = serde_json::to_vec(request).expect("serialize request");
            match vector::parse_query(&body, &mut query) {
                Ok(()) => {
                    let (count, parts) = index.predict_fraud_count_with_partitions(&query);
                    let part_str = parts
                        .keys_sorted()
                        .iter()
                        .map(|k| k.to_string())
                        .collect::<Vec<_>>()
                        .join("|");
                    write_row(&mut out, &query, count, &part_str);
                    total_ok += 1;
                }
                Err(e) => {
                    total_err += 1;
                    if total_err <= 5 {
                        eprintln!("parse error {} entry {}: {:?}", path, i, e);
                    }
                }
            }
            if (i + 1) % 10000 == 0 {
                eprintln!("  {} / {} ...", i + 1, n);
            }
        }
    }

    eprintln!("Done: ok={} err={} -> {}", total_ok, total_err, out_path);
}

fn write_row(out: &mut std::fs::File, query: &QueryVector, count: u8, partitions: &str) {
    let mut line = String::with_capacity(128);
    for d in 0..DIMS {
        if d > 0 {
            line.push(',');
        }
        line.push_str(&query[d].to_string());
    }
    line.push(',');
    line.push_str(&count.to_string());
    line.push(',');
    line.push_str(partitions);
    line.push('\n');
    out.write_all(line.as_bytes()).expect("write row");
}
