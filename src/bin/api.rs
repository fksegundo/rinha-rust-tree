fn main() {
    let index_path = std::env::var("RINHA_INDEX_PATH")
        .unwrap_or_else(|_| "/app/index/rinha-specialist.idx".to_string());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let fd_socket = std::env::var("RINHA_FD_SOCKET").ok();

    rinha_rust_tree::api::run(&index_path, &bind_addr, fd_socket.as_deref());
}
