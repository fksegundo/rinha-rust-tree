pub fn warmup_queries() -> usize {
    std::env::var("RINHA_WARMUP_QUERIES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(256)
}

pub fn payload_warmup_requests() -> usize {
    std::env::var("RINHA_PAYLOAD_WARMUP_REQUESTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(256)
}

pub fn self_warmup_enabled() -> bool {
    std::env::var("RINHA_SELF_WARMUP").as_deref() == Ok("1")
}

pub fn self_warmup_url() -> String {
    std::env::var("RINHA_SELF_WARMUP_URL")
        .unwrap_or_else(|_| "http://localhost:9999/fraud-score".to_string())
}

pub fn self_warmup_duration_ms() -> u64 {
    std::env::var("RINHA_SELF_WARMUP_DURATION_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(45_000)
}

pub fn self_warmup_concurrency() -> usize {
    std::env::var("RINHA_SELF_WARMUP_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(4)
}

pub fn self_warmup_payloads_path() -> String {
    std::env::var("RINHA_SELF_WARMUP_PAYLOADS")
        .unwrap_or_else(|_| "/app/resources/warmup-payloads.jsonl".to_string())
}
