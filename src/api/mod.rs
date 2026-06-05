mod handler;
mod server;
mod warmup;

use crate::index::SpecialistIndex;
use crate::runtime;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
    warmup::warm_up_index(&index);
    eprintln!(
        "warming up payload path with {} requests...",
        runtime::payload_warmup_requests()
    );
    warmup::warm_up_payload_path(&index);

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
            server::log_self_warmup_config();
            server::set_accept_warmup(true);
            eprintln!(
                "accepting worker sockets at {prefix}.sock (defer /ready until self HTTP warmup)"
            );
            server::run_fd_worker_mode(index, ready, prefix, true);
        } else {
            ready.store(true, std::sync::atomic::Ordering::Release);
            eprintln!("warmup complete, accepting worker sockets at {prefix}.sock");
            server::run_fd_worker_mode(index, ready, prefix, false);
        }
        return;
    }

    let socket_path =
        fd_socket.expect("FD-passing socket required: set RINHA_FD_SOCKET or mount /sockets");

    if runtime::self_warmup_enabled() {
        server::log_self_warmup_config();
        server::set_accept_warmup(true);
        server::run_fd_mode_evented_with_self_warmup(index, ready, socket_path.to_string());
    } else {
        ready.store(true, std::sync::atomic::Ordering::Release);
        eprintln!("warmup complete, accepting connections");
        server::run_fd_mode(index, ready, socket_path);
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
