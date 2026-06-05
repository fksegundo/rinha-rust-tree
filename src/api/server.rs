use crate::index::SpecialistIndex;
use crate::runtime;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

static ACCEPT_WARMUP: AtomicBool = AtomicBool::new(false);

pub fn run_fd_worker_mode(
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
                        super::handler::handle_request(req, &index, &ready)
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

pub fn run_fd_mode_evented_with_self_warmup(
    index: Arc<SpecialistIndex>,
    ready: Arc<AtomicBool>,
    socket_path: String,
) {
    eprintln!(
        "starting FD evented server on {} (defer /ready until self HTTP warmup)",
        socket_path
    );
    let ready_hook = Arc::clone(&ready);
    crate::fd_passing::run_fd_evented_server_with_hook(
        &socket_path,
        move |req| super::handler::handle_request(req, &index, &ready),
        Some(move || {
            start_self_warmup_thread(ready_hook);
        }),
    );
}

pub fn run_fd_mode(index: Arc<SpecialistIndex>, ready: Arc<AtomicBool>, socket_path: &str) {
    eprintln!("starting FD evented server on {}", socket_path);
    crate::fd_passing::run_fd_evented_server(socket_path, move |req| {
        super::handler::handle_request(req, &index, &ready)
    });
}

fn start_self_warmup_thread(ready: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        super::warmup::warm_up_via_http(
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

pub fn log_self_warmup_config() {
    eprintln!(
        "self HTTP warmup enabled (url={} duration={}ms concurrency={} payloads={})",
        runtime::self_warmup_url(),
        runtime::self_warmup_duration_ms(),
        runtime::self_warmup_concurrency(),
        runtime::self_warmup_payloads_path()
    );
}

pub fn set_accept_warmup(value: bool) {
    ACCEPT_WARMUP.store(value, Ordering::Release);
}

pub fn accept_warmup() -> bool {
    ACCEPT_WARMUP.load(Ordering::Relaxed)
}
