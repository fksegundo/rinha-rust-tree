use crate::http;
use crate::index::SpecialistIndex;
use crate::runtime;
use crate::vector;
use crate::{PACKED_DIMS, SCALE};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static ACCEPT_WARMUP: AtomicBool = AtomicBool::new(false);

pub fn run(index_path: &str, fd_socket: Option<&str>) {
    let index = Arc::new(
        SpecialistIndex::open(index_path)
            .unwrap_or_else(|e| panic!("failed to open index '{}': {}", index_path, e)),
    );

    if std::env::var("RINHA_MLOCK_INDEX").as_deref() == Ok("1") {
        index.mlock_all();
    }
    if std::env::var("RINHA_PRETOUCH_INDEX").as_deref() == Ok("1") {
        index.pretouch_all();
    }

    let ready = Arc::new(AtomicBool::new(false));

    eprintln!(
        "warming up index with {} queries...",
        runtime::warmup_queries()
    );
    warm_up_index(&index);
    eprintln!(
        "warming up payload path with {} requests...",
        runtime::payload_warmup_requests()
    );
    warm_up_payload_path(&index);

    let api_socket_prefix = std::env::var("API_SOCKET_PREFIX").ok().or_else(|| {
        if std::path::Path::new("/sockets").is_dir() {
            let hostname = std::env::var("HOSTNAME").unwrap_or_default();
            if !hostname.is_empty() {
                Some(format!("/sockets/{}", hostname))
            } else {
                Some("/sockets/api1".to_string())
            }
        } else {
            None
        }
    });

    if let Some(prefix) = api_socket_prefix {
        if runtime::self_warmup_enabled() {
            log_self_warmup_config();
            ACCEPT_WARMUP.store(true, Ordering::Release);
            eprintln!(
                "accepting worker sockets at {prefix}.sock (defer /ready until self HTTP warmup)"
            );
            run_fd_worker_mode(index, ready, prefix, true);
        } else {
            ready.store(true, Ordering::Release);
            eprintln!("warmup complete, accepting worker sockets at {prefix}.sock");
            run_fd_worker_mode(index, ready, prefix, false);
        }
        return;
    }

    let socket_path =
        fd_socket.expect("FD-passing socket required: set RINHA_FD_SOCKET or mount /sockets");

    if runtime::self_warmup_enabled() {
        log_self_warmup_config();
        ACCEPT_WARMUP.store(true, Ordering::Release);
        run_fd_mode_evented_with_self_warmup(index, ready, socket_path.to_string());
    } else {
        ready.store(true, Ordering::Release);
        eprintln!("warmup complete, accepting connections");
        run_fd_mode(index, ready, socket_path);
    }
}

fn log_self_warmup_config() {
    eprintln!(
        "self HTTP warmup enabled (url={} duration={}ms concurrency={} payloads={})",
        runtime::self_warmup_url(),
        runtime::self_warmup_duration_ms(),
        runtime::self_warmup_concurrency(),
        runtime::self_warmup_payloads_path()
    );
}

fn run_fd_worker_mode(
    index: Arc<SpecialistIndex>,
    ready: Arc<AtomicBool>,
    socket_prefix: String,
    self_warmup: bool,
) {
    let workers = std::env::var("API_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);

    let mut handles = Vec::with_capacity(workers);
    for worker in 0..workers {
        let socket_path = if workers == 1 {
            format!("{socket_prefix}.sock")
        } else {
            format!("{socket_prefix}-w{worker}.sock")
        };
        let index = Arc::clone(&index);
        let ready = Arc::clone(&ready);
        handles.push(
            std::thread::Builder::new()
                .name(format!("api-worker-{worker}"))
                .spawn(move || {
                    eprintln!("starting FD worker {worker} on {socket_path}");
                    crate::fd_passing::run_fd_evented_server(&socket_path, move |req| {
                        handle_request(req, &index, &ready)
                    });
                })
                .expect("spawn api worker"),
        );
    }

    if self_warmup {
        start_self_warmup_thread(ready);
    }

    for handle in handles {
        let _ = handle.join();
    }
}

fn start_self_warmup_thread(ready: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        warm_up_via_http(
            &runtime::self_warmup_url(),
            runtime::self_warmup_duration_ms(),
            runtime::self_warmup_concurrency(),
            &runtime::self_warmup_payloads_path(),
        );
        ACCEPT_WARMUP.store(false, Ordering::Release);
        ready.store(true, Ordering::Release);
        eprintln!("self HTTP warmup complete, /ready");
    });
}

fn run_fd_mode_evented_with_self_warmup(
    index: Arc<SpecialistIndex>,
    ready: Arc<AtomicBool>,
    socket_path: String,
) {
    use crate::fd_passing;
    eprintln!(
        "starting FD evented server on {} (defer /ready until self HTTP warmup)",
        socket_path
    );
    let ready_hook = Arc::clone(&ready);
    fd_passing::run_fd_evented_server_with_hook(
        &socket_path,
        move |req| handle_request(req, &index, &ready),
        Some(move || {
            start_self_warmup_thread(ready_hook);
        }),
    );
}

fn run_fd_mode(index: Arc<SpecialistIndex>, ready: Arc<AtomicBool>, socket_path: &str) {
    use crate::fd_passing;
    eprintln!("starting FD evented server on {}", socket_path);
    fd_passing::run_fd_evented_server(socket_path, move |req| handle_request(req, &index, &ready));
}

fn warm_up_index(index: &SpecialistIndex) {
    let count = runtime::warmup_queries();
    let scale = SCALE as usize;
    for i in 0..count {
        let mut query = [0i16; PACKED_DIMS];
        for (dim, value) in query.iter_mut().enumerate() {
            let raw = ((i * 313 + dim * 1009) % (scale + 1)) as i16;
            *value = if (dim == 5 || dim == 6) && i % 4 == 0 {
                -(SCALE as i16)
            } else {
                raw
            };
        }
        let _ = index.predict_fraud_count(&query);
    }
}

fn warm_up_payload_path(index: &SpecialistIndex) {
    let count = runtime::payload_warmup_requests();
    if count == 0 {
        return;
    }

    let ready = AtomicBool::new(true);
    for i in 0..count {
        let body = WARMUP_PAYLOADS[i % WARMUP_PAYLOADS.len()];
        let mut request = Vec::with_capacity(body.len() + 96);
        warm_up_payload_body(index, &ready, body, &mut request);
    }
}

fn warm_up_payload_body(
    index: &SpecialistIndex,
    ready: &AtomicBool,
    body: &[u8],
    request: &mut Vec<u8>,
) {
    request.clear();
    request.extend_from_slice(b"POST /fraud-score HTTP/1.1\r\nHost: localhost\r\nContent-Length: ");
    request.extend_from_slice(body.len().to_string().as_bytes());
    request.extend_from_slice(b"\r\n\r\n");
    request.extend_from_slice(body);

    if let Some((req, _)) = http::parse_request(request) {
        let _ = handle_request(&req, index, ready);
    }
}

#[derive(Clone)]
struct WarmupHttpTarget {
    connect_addr: String,
    host_header: String,
    path: String,
}

fn warm_up_via_http(url: &str, duration_ms: u64, concurrency: usize, payloads_path: &str) {
    if duration_ms == 0 || concurrency == 0 {
        return;
    }
    let target = match parse_warmup_url(url) {
        Some(target) => target,
        None => {
            eprintln!("self HTTP warmup: invalid URL '{}'", url);
            return;
        }
    };
    let deadline = Instant::now() + Duration::from_millis(duration_ms);
    let mut handles = Vec::with_capacity(concurrency);
    for worker in 0..concurrency {
        let target = target.clone();
        let payloads_path = payloads_path.to_string();
        handles.push(
            std::thread::Builder::new()
                .name(format!("self-warmup-{worker}"))
                .spawn(move || {
                    warm_up_via_http_worker(worker, concurrency, &target, deadline, &payloads_path)
                })
                .expect("spawn self HTTP warmup worker"),
        );
    }

    let mut sent = 0usize;
    for handle in handles {
        sent += handle.join().unwrap_or(0);
    }
    eprintln!("self HTTP warmup: sent {} requests", sent);
}

fn warm_up_via_http_worker(
    worker: usize,
    concurrency: usize,
    target: &WarmupHttpTarget,
    deadline: Instant,
    payloads_path: &str,
) -> usize {
    let mut stream = match connect_warmup_target(target, deadline) {
        Some(stream) => stream,
        None => return 0,
    };
    let mut request = Vec::with_capacity(4096);
    let mut response = Vec::with_capacity(256);
    let mut sent = 0usize;

    if std::path::Path::new(payloads_path).is_file() {
        while Instant::now() < deadline {
            let file = match std::fs::File::open(payloads_path) {
                Ok(file) => file,
                Err(err) => {
                    eprintln!("self HTTP warmup: open {} failed: {}", payloads_path, err);
                    break;
                }
            };
            for (line_no, line) in BufReader::new(file).lines().enumerate() {
                if Instant::now() >= deadline {
                    break;
                }
                if line_no % concurrency != worker {
                    continue;
                }
                let line = match line {
                    Ok(line) => line,
                    Err(_) => continue,
                };
                let body = line.trim().as_bytes();
                if body.is_empty() {
                    continue;
                }
                if !send_warmup_http_request(target, body, &mut request, &mut response, &mut stream)
                {
                    stream = match connect_warmup_target(target, deadline) {
                        Some(stream) => stream,
                        None => return sent,
                    };
                    if send_warmup_http_request(
                        target,
                        body,
                        &mut request,
                        &mut response,
                        &mut stream,
                    ) {
                        sent += 1;
                    }
                } else {
                    sent += 1;
                }
            }
        }
        return sent;
    }

    eprintln!(
        "self HTTP warmup: payload file {} missing; using built-in fallback payloads",
        payloads_path
    );
    let mut idx = worker;
    while Instant::now() < deadline {
        let body = WARMUP_PAYLOADS[idx % WARMUP_PAYLOADS.len()];
        if !send_warmup_http_request(target, body, &mut request, &mut response, &mut stream) {
            stream = match connect_warmup_target(target, deadline) {
                Some(stream) => stream,
                None => return sent,
            };
            continue;
        }
        sent += 1;
        idx += 1;
    }
    sent
}

fn parse_warmup_url(url: &str) -> Option<WarmupHttpTarget> {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/fraud-score".to_string()),
    };
    if authority.is_empty() || path.is_empty() {
        return None;
    }
    let host_header = authority.to_string();
    let connect_addr = if authority.rsplit_once(':').is_some() {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    Some(WarmupHttpTarget {
        connect_addr,
        host_header,
        path,
    })
}

fn connect_warmup_target(target: &WarmupHttpTarget, deadline: Instant) -> Option<TcpStream> {
    let connect_deadline = std::cmp::min(Instant::now() + Duration::from_secs(10), deadline);
    let mut backoff = Duration::from_millis(25);
    while Instant::now() < connect_deadline {
        match TcpStream::connect(&target.connect_addr) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(800)));
                return Some(stream);
            }
            Err(_) => {
                std::thread::sleep(backoff);
                backoff = std::cmp::min(backoff * 2, Duration::from_millis(250));
            }
        }
    }
    eprintln!(
        "self HTTP warmup: could not connect to {}",
        target.connect_addr
    );
    None
}

fn send_warmup_http_request(
    target: &WarmupHttpTarget,
    body: &[u8],
    request: &mut Vec<u8>,
    response: &mut Vec<u8>,
    stream: &mut TcpStream,
) -> bool {
    build_warmup_http_post(
        body,
        target.path.as_bytes(),
        target.host_header.as_bytes(),
        request,
    );
    if stream.write_all(request).is_err() {
        let _ = stream.shutdown(Shutdown::Both);
        return false;
    }
    read_warmup_http_response(stream, response).is_ok()
}

fn read_warmup_http_response(
    stream: &mut TcpStream,
    response: &mut Vec<u8>,
) -> std::io::Result<()> {
    response.clear();
    let mut scratch = [0u8; 256];
    let header_end = loop {
        if let Some(pos) = find_header_end_bytes(response) {
            break pos;
        }
        let n = stream.read(&mut scratch)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        response.extend_from_slice(&scratch[..n]);
        if response.len() > 1024 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
    };
    let content_length = parse_response_content_length(&response[..header_end]).unwrap_or(0);
    let needed = header_end + content_length;
    while response.len() < needed {
        let n = stream.read(&mut scratch)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        response.extend_from_slice(&scratch[..n]);
    }
    Ok(())
}

fn find_header_end_bytes(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn parse_response_content_length(headers: &[u8]) -> Option<usize> {
    for line in headers.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(value) = line.strip_prefix(b"Content-Length:") {
            let value = std::str::from_utf8(value).ok()?.trim();
            return value.parse().ok();
        }
    }
    None
}

fn build_warmup_http_post(body: &[u8], path: &[u8], host: &[u8], req: &mut Vec<u8>) {
    req.clear();
    req.extend_from_slice(b"POST ");
    req.extend_from_slice(path);
    req.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    req.extend_from_slice(host);
    req.extend_from_slice(b"\r\nContent-Length: ");
    req.extend_from_slice(body.len().to_string().as_bytes());
    req.extend_from_slice(b"\r\n\r\n");
    req.extend_from_slice(body);
}

const WARMUP_PAYLOADS: &[&[u8]] = &[
    // --- LEGIT profile: low amount, low installments, low km_from_home, known merchant ---
    br#"{"id":"warmup-1","transaction":{"amount":441.59,"installments":1,"requested_at":"2027-07-09T16:31:06Z"},"customer":{"avg_amount":883.18,"tx_count_24h":1,"known_merchants":["MERC-004","MERC-017"]},"merchant":{"id":"MERC-004","mcc":"5411","avg_amount":302.78},"terminal":{"is_online":false,"card_present":true,"km_from_home":33.88},"last_transaction":{"timestamp":"2027-06-04T14:14:22Z","km_from_current":18.43}}"#,
    br#"{"id":"warmup-5","transaction":{"amount":29.47,"installments":2,"requested_at":"2028-12-24T08:34:05Z"},"customer":{"avg_amount":58.94,"tx_count_24h":3,"known_merchants":["MERC-004","MERC-014"]},"merchant":{"id":"MERC-014","mcc":"5411","avg_amount":378.62},"terminal":{"is_online":false,"card_present":true,"km_from_home":20.36},"last_transaction":{"timestamp":"2027-11-28T15:22:55Z","km_from_current":16.71}}"#,
    // legit with mcc=5912 (pharmacy), is_online=true
    br#"{"id":"warmup-7","transaction":{"amount":87.30,"installments":1,"requested_at":"2027-01-15T10:20:00Z"},"customer":{"avg_amount":174.60,"tx_count_24h":2,"known_merchants":["MERC-002","MERC-011"]},"merchant":{"id":"MERC-002","mcc":"5912","avg_amount":95.44},"terminal":{"is_online":true,"card_present":true,"km_from_home":5.20},"last_transaction":{"timestamp":"2027-01-14T18:05:00Z","km_from_current":3.10}}"#,
    // legit with mcc=5541 (gas station), card_present=true
    br#"{"id":"warmup-8","transaction":{"amount":210.00,"installments":3,"requested_at":"2027-03-22T07:45:00Z"},"customer":{"avg_amount":420.00,"tx_count_24h":1,"known_merchants":["MERC-019"]},"merchant":{"id":"MERC-019","mcc":"5541","avg_amount":188.50},"terminal":{"is_online":false,"card_present":true,"km_from_home":12.40},"last_transaction":{"timestamp":"2027-03-21T19:30:00Z","km_from_current":8.70}}"#,
    // legit with last_transaction=null (~20% of real data)
    br#"{"id":"warmup-9","transaction":{"amount":150.00,"installments":1,"requested_at":"2027-05-10T14:00:00Z"},"customer":{"avg_amount":300.00,"tx_count_24h":1,"known_merchants":["MERC-006"]},"merchant":{"id":"MERC-006","mcc":"5411","avg_amount":250.00},"terminal":{"is_online":false,"card_present":true,"km_from_home":8.50},"last_transaction":null}"#,
    // --- FRAUD profile: high amount, high installments, high km, unknown merchant ---
    br#"{"id":"warmup-2","transaction":{"amount":5293.06,"installments":8,"requested_at":"2028-09-19T03:34:29Z"},"customer":{"avg_amount":60.14,"tx_count_24h":11,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-087","mcc":"7995","avg_amount":21.57},"terminal":{"is_online":false,"card_present":false,"km_from_home":265.78},"last_transaction":{"timestamp":"2024-01-04T03:43:32Z","km_from_current":722.93}}"#,
    br#"{"id":"warmup-3","transaction":{"amount":7318.26,"installments":8,"requested_at":"2028-07-05T03:41:22Z"},"customer":{"avg_amount":158.57,"tx_count_24h":11,"known_merchants":["MERC-013","MERC-010"]},"merchant":{"id":"MERC-073","mcc":"7801","avg_amount":37.46},"terminal":{"is_online":true,"card_present":false,"km_from_home":417.33},"last_transaction":null}"#,
    br#"{"id":"warmup-6","transaction":{"amount":9797.7,"installments":7,"requested_at":"2026-11-14T06:09:00Z"},"customer":{"avg_amount":99.49,"tx_count_24h":13,"known_merchants":["MERC-006","MERC-014","MERC-013"]},"merchant":{"id":"MERC-094","mcc":"7802","avg_amount":33.01},"terminal":{"is_online":false,"card_present":true,"km_from_home":396.12},"last_transaction":{"timestamp":"2026-03-18T15:14:27Z","km_from_current":712.42}}"#,
    // fraud with mcc=4121 (taxi/limo), online, high tx_count
    br#"{"id":"warmup-10","transaction":{"amount":8500.00,"installments":12,"requested_at":"2027-08-03T02:15:00Z"},"customer":{"avg_amount":45.00,"tx_count_24h":15,"known_merchants":["MERC-003"]},"merchant":{"id":"MERC-099","mcc":"4121","avg_amount":18.00},"terminal":{"is_online":true,"card_present":false,"km_from_home":650.00},"last_transaction":{"timestamp":"2027-08-02T23:50:00Z","km_from_current":900.00}}"#,
    // fraud with last_transaction=null, mcc=5999
    br#"{"id":"warmup-11","transaction":{"amount":6200.00,"installments":10,"requested_at":"2027-06-18T04:30:00Z"},"customer":{"avg_amount":120.00,"tx_count_24h":9,"known_merchants":["MERC-005"]},"merchant":{"id":"MERC-077","mcc":"5999","avg_amount":42.00},"terminal":{"is_online":true,"card_present":false,"km_from_home":520.00},"last_transaction":null}"#,
    // --- BORDERLINE profile: mid amount, mid installments, mid km ---
    br#"{"customer":{"avg_amount":68.88,"tx_count_24h":18,"known_merchants":["MERC-004","MERC-015","MERC-007"]},"id":"warmup-4","last_transaction":{"timestamp":"2026-03-17T01:58:06Z","km_from_current":660.92},"merchant":{"id":"MERC-062","mcc":"7801","avg_amount":25.55},"terminal":{"is_online":true,"card_present":false,"km_from_home":881.61},"transaction":{"amount":4368.82,"installments":8,"requested_at":"2026-03-17T02:04:06Z"}}"#,
    // borderline with mid-range values, mcc=7995
    br#"{"id":"warmup-12","transaction":{"amount":1200.00,"installments":5,"requested_at":"2027-04-12T11:00:00Z"},"customer":{"avg_amount":300.00,"tx_count_24h":5,"known_merchants":["MERC-010","MERC-018"]},"merchant":{"id":"MERC-045","mcc":"7995","avg_amount":80.00},"terminal":{"is_online":false,"card_present":true,"km_from_home":120.00},"last_transaction":{"timestamp":"2027-04-11T22:00:00Z","km_from_current":95.00}}"#,
    // borderline with card_present=false, is_online=true, mcc=7802
    br#"{"id":"warmup-13","transaction":{"amount":2500.00,"installments":6,"requested_at":"2027-09-05T22:30:00Z"},"customer":{"avg_amount":500.00,"tx_count_24h":7,"known_merchants":["MERC-012"]},"merchant":{"id":"MERC-055","mcc":"7802","avg_amount":150.00},"terminal":{"is_online":true,"card_present":false,"km_from_home":250.00},"last_transaction":{"timestamp":"2027-09-05T20:00:00Z","km_from_current":180.00}}"#,
    // borderline with last_transaction=null, mcc=4121
    br#"{"id":"warmup-14","transaction":{"amount":800.00,"installments":4,"requested_at":"2027-11-20T15:00:00Z"},"customer":{"avg_amount":200.00,"tx_count_24h":4,"known_merchants":["MERC-020","MERC-003"]},"merchant":{"id":"MERC-030","mcc":"4121","avg_amount":65.00},"terminal":{"is_online":false,"card_present":true,"km_from_home":80.00},"last_transaction":null}"#,
    // borderline with very different field order (tests parser robustness)
    br#"{"transaction":{"amount":1800.00,"installments":7,"requested_at":"2027-02-28T08:15:00Z"},"id":"warmup-15","terminal":{"is_online":true,"card_present":true,"km_from_home":200.00},"merchant":{"id":"MERC-041","mcc":"5912","avg_amount":110.00},"customer":{"avg_amount":400.00,"tx_count_24h":6,"known_merchants":["MERC-041","MERC-008"]},"last_transaction":{"timestamp":"2027-02-27T16:30:00Z","km_from_current":350.00}}"#,
    // --- Edge cases: extreme values ---
    // very low amount, mcc=5541, card_present=true, known merchant
    br#"{"id":"warmup-16","transaction":{"amount":10.50,"installments":1,"requested_at":"2027-12-01T12:00:00Z"},"customer":{"avg_amount":21.00,"tx_count_24h":1,"known_merchants":["MERC-019"]},"merchant":{"id":"MERC-019","mcc":"5541","avg_amount":400.00},"terminal":{"is_online":false,"card_present":true,"km_from_home":2.00},"last_transaction":{"timestamp":"2027-11-30T10:00:00Z","km_from_current":1.50}}"#,
    // very high amount, mcc=7801, all flags hostile
    br#"{"id":"warmup-17","transaction":{"amount":9999.99,"installments":13,"requested_at":"2027-07-04T01:00:00Z"},"customer":{"avg_amount":30.00,"tx_count_24h":20,"known_merchants":["MERC-001"]},"merchant":{"id":"MERC-098","mcc":"7801","avg_amount":15.00},"terminal":{"is_online":true,"card_present":false,"km_from_home":999.00},"last_transaction":{"timestamp":"2027-07-03T23:00:00Z","km_from_current":950.00}}"#,
    // zero-ish amount with mcc=5999
    br#"{"id":"warmup-18","transaction":{"amount":500.00,"installments":4,"requested_at":"2027-10-15T16:00:00Z"},"customer":{"avg_amount":250.00,"tx_count_24h":3,"known_merchants":["MERC-007","MERC-022"]},"merchant":{"id":"MERC-007","mcc":"5999","avg_amount":200.00},"terminal":{"is_online":false,"card_present":true,"km_from_home":45.00},"last_transaction":{"timestamp":"2027-10-15T10:00:00Z","km_from_current":30.00}}"#,
];

fn is_ready(ready: &AtomicBool) -> bool {
    ready.load(Ordering::Acquire)
}

fn can_serve_fraud(ready: &AtomicBool) -> bool {
    is_ready(ready) || ACCEPT_WARMUP.load(Ordering::Relaxed)
}

fn handle_request(
    req: &http::Request,
    index: &SpecialistIndex,
    ready: &AtomicBool,
) -> &'static [u8] {
    match req.method {
        http::Method::Get if req.path == b"/ready" => {
            if is_ready(ready) {
                http::RESPONSE_READY
            } else {
                http::RESPONSE_NOT_READY
            }
        }
        http::Method::Post if req.path == b"/fraud-score" => {
            if !can_serve_fraud(ready) {
                return http::RESPONSE_NOT_READY;
            }
            let mut query = [0i16; 16];
            match vector::parse_query(req.body, &mut query) {
                Ok(()) => {
                    let count = index.predict_fraud_count(&query) as usize;
                    if count < http::FRAUD_RESPONSES.len() {
                        http::FRAUD_RESPONSES[count]
                    } else {
                        http::FRAUD_RESPONSES[5]
                    }
                }
                Err(_) => http::RESPONSE_BAD_REQUEST,
            }
        }
        _ => http::RESPONSE_NOT_FOUND,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_api_socket_prefix_behavior() {
        unsafe {
            std::env::remove_var("API_SOCKET_PREFIX");
            std::env::remove_var("HOSTNAME");
        }

        let prefix_no_sockets = std::env::var("API_SOCKET_PREFIX").ok().or_else(|| {
            if std::path::Path::new("/sockets").is_dir() {
                let hostname = std::env::var("HOSTNAME").unwrap_or_default();
                if !hostname.is_empty() {
                    Some(format!("/sockets/{}", hostname))
                } else {
                    Some("/sockets/api1".to_string())
                }
            } else {
                None
            }
        });
        if !std::path::Path::new("/sockets").is_dir() {
            assert_eq!(prefix_no_sockets, None);
        }

        unsafe {
            std::env::set_var("API_SOCKET_PREFIX", "/custom/path");
        }
        let prefix_with_env = std::env::var("API_SOCKET_PREFIX").ok().or_else(|| {
            if std::path::Path::new("/sockets").is_dir() {
                let hostname = std::env::var("HOSTNAME").unwrap_or_default();
                if !hostname.is_empty() {
                    Some(format!("/sockets/{}", hostname))
                } else {
                    Some("/sockets/api1".to_string())
                }
            } else {
                None
            }
        });
        assert_eq!(prefix_with_env, Some("/custom/path".to_string()));

        unsafe {
            std::env::remove_var("API_SOCKET_PREFIX");
        }
    }
}
