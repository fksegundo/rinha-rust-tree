//! Print index metadata as one JSON line.
//! Usage: index_info <path.idx>

use rinha_rust_tree::index::SpecialistIndex;
use std::env;

fn main() {
    let path = env::args().nth(1).expect("usage: index_info <path.idx>");
    let index = SpecialistIndex::open(&path).expect("open index");
    let m = index.metadata();
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        r#"{{"path":"{path}","file_bytes":{bytes},"references":{refs},"partitions":{parts},"nodes":{nodes},"blocks":{blocks}}}"#,
        path = path,
        bytes = bytes,
        refs = m.reference_count,
        parts = m.partition_count,
        nodes = m.node_count,
        blocks = m.block_count,
    );
}
