# Rinha Rust - Low-Latency Fraud Scoring

[![Rust CI](https://github.com/fksegundo/rinha-rust-tree/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/fksegundo/rinha-rust-tree/actions/workflows/rust-ci.yml)
[![Build image](https://github.com/fksegundo/rinha-rust-tree/actions/workflows/publish-image.yml/badge.svg)](https://github.com/fksegundo/rinha-rust-tree/actions/workflows/publish-image.yml)
[![GHCR image](https://img.shields.io/badge/GHCR-rinha--rust--tree--api-blue)](https://github.com/fksegundo/rinha-rust-tree/pkgs/container/rinha-rust-tree-api)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

High-performance Rust implementation for the [Rinha de Backend 2026](https://github.com/zanfranceschi/rinha-de-backend-2026) challenge, featuring low-latency fraud scoring with vector similarity search.

[🇧🇷 Leia em Português](README.pt.md)

## Overview

This project implements a fraud detection API that uses k-nearest neighbors (k-NN) search on quantized feature vectors. The system is optimized for low latency through:

- **AVX2 SIMD** for vectorized distance calculations
- **Memory-mapped indexes** for zero-copy loading
- **Epoll-based event loop** for efficient I/O multiplexing
- **File descriptor passing** for inter-process communication
- **Learned tree partitioning** for efficient index search
- **Specialized JSON parsers** for fast request parsing

## Architecture

### Core Components

- **`src/api/`** - HTTP API server with warmup and request handling
- **`src/http/`** - HTTP request/response parsing
- **`src/fd_passing/`** - File descriptor passing with epoll event loop
- **`src/index/`** - Vector index with tree-based partitioning
- **`src/vector/`** - Query vector parsing with multiple strategies
- **`src/runtime/`** - Runtime configuration from environment variables
- **`src/lb/`** - Load balancer for multi-instance deployment

### Module Organization

```
src/
├── api/
│   ├── mod.rs          # Main entry point
│   ├── warmup.rs       # Index warmup logic
│   ├── server.rs       # FD mode server
│   └── handler.rs      # HTTP request handler
├── http/
│   ├── mod.rs          # HTTP module entry
│   ├── parser.rs       # Request parsing
│   └── responses.rs    # Response constants
├── fd_passing/
│   ├── mod.rs          # FD passing entry
│   ├── evented.rs      # Event loop logic
│   ├── conn.rs         # Connection management
│   ├── epoll.rs        # Epoll operations
│   └── io.rs           # I/O utilities
├── index/
│   ├── mod.rs          # Index entry
│   ├── build.rs        # Index building
│   ├── format.rs       # Index format
│   ├── layout.rs       # Memory layout
│   ├── partition_scheme.rs  # Tree partitioning
│   ├── mmap.rs         # Memory mapping
│   └── search.rs       # Search algorithms
├── vector/
│   ├── mod.rs          # Vector parsing entry
│   ├── helpers.rs      # Helper functions
│   ├── compact.rs      # Compact ordered parser
│   ├── single_pass.rs  # Single-pass parser
│   └── serde_fallback.rs  # Serde fallback parser
└── runtime.rs          # Runtime configuration
```

## Technologies

- **Rust 2024 Edition** - Modern Rust with latest language features
- **libc** - Direct system calls for epoll, mmap, socket operations
- **AVX2 SIMD** - Vectorized distance calculations via `std::arch::x86_64`
- **mimalloc** - High-performance memory allocator
- **serde_json** - Fallback JSON parsing
- **threadpool** - Thread pool for concurrent operations
- **flate2** - Compression support

## Algorithms

### k-NN Search with Tree Partitioning

The index uses a learned decision tree to partition the vector space into 256 buckets (Tree256). Each query is routed to the most relevant partitions based on tree predicates, then k-NN search is performed within those partitions.

**Key features:**
- **Label deferral optimization** - Skips searching subtrees when consensus is reached
- **Early exit threshold** - Stops search when k-th neighbor distance is below threshold
- **AVX2-accelerated distance** - Computes 8 distances in parallel using SIMD
- **Lower bound pruning** - Uses bounding boxes to skip irrelevant partitions

### Partition Scheme

The partition scheme is learned from sample queries using a decision tree:
- **Tree depth**: 8 levels (configurable up to 10)
- **Predicates**: Learned thresholds on vector dimensions
- **Key computation**: Binary traversal produces 8-bit partition key

### JSON Parsing Strategies

Three parsing strategies are tried in order of performance:

1. **Compact Ordered Parser** - Assumes fixed field order, fastest path
2. **Single-Pass Parser** - Handles any field order with skipping
3. **Serde Fallback** - Full serde deserialization for compatibility

### Distance Calculation

Squared Euclidean distance computed with AVX2:
- Pairs of dimensions processed in parallel
- Early rejection using distance bounds
- Quantized i16 values for cache efficiency

## Environment Variables

### Index Configuration
- `RINHA_INDEX_PATH` - Path to the index file
- `RINHA_NATIVE_SCALE` - Quantization scale (build-time)
- `RINHA_EARLY_EXIT_THRESHOLD` - Stop search when k-th distance below this value
- `RINHA_LABEL_DEFER` - Enable label deferral optimization (0/1)

### Runtime Configuration
- `RINHA_WARMUP_QUERIES` - Number of warmup queries
- `RINHA_SELF_WARMUP_URL` - URL for self-warmup
- `RINHA_SELF_WARMUP_DURATION_MS` - Self-warmup duration
- `RINHA_SELF_WARMUP_CONCURRENCY` - Self-warmup concurrency
- `RINHA_WARMUP_PAYLOADS_PATH` - Path to warmup payloads

### Epoll Configuration
- `RINHA_EPOLL_BUSY_POLL` - Enable busy polling (0/1)
- `RINHA_EPOLL_IDLE_US` - Idle timeout in microseconds
- `RINHA_SPIN_BEFORE_BLOCK_US` - Spin duration before blocking

### Socket Configuration
- `RINHA_CLIENT_FD_PRECONFIGURED` - Assume FDs are pre-configured (0/1)

## Building

```bash
# Build release binaries
cargo build --release --bin api --bin preprocess --bin lb

# Build Docker images
make build

# Validate Docker Compose configuration
make config
```

## Running

```bash
# Start local stack
make up

# Stop local stack
make down
```

## Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test --lib vector::tests::tests::compact_ordered_matches_serde_fallback
```

## Performance Optimizations

### Memory
- **Memory-mapped indexes** - Zero-copy loading with `mmap`
- **Huge page advice** - `MADV_HUGEPAGE` for TLB efficiency
- **mimalloc allocator** - Reduced fragmentation
- **Quantized vectors** - i16 instead of f64 for cache efficiency

### CPU
- **AVX2 SIMD** - Parallel distance calculations
- **Busy polling** - Reduce latency when under load
- **Label deferral** - Skip unnecessary subtree searches
- **Early exit** - Stop search when result is confident

### I/O
- **Epoll edge-triggered** - Efficient event notification
- **Non-blocking sockets** - `TCP_NODELAY`, non-blocking mode
- **File descriptor passing** - Zero-copy between processes
- **Buffer pooling** - Reuse buffers to reduce allocations

## Index Format

The index uses a custom binary format (V5):

```
Header:
- Magic: "RNSPCST5" (8 bytes)
- Scale: i32
- Packed dimensions: i32
- Reference count: i32
- Partition count: i32
- Node count: i32
- Block count: i32
- Partition scheme: i16 (scheme_id, param, cut counts)
- Tree predicates: [dim: u8, flags: u8, threshold: i16]

Data sections:
- Partitions: [key: u16, root: u32, min: [i16; 16], max: [i16; 16]]
- Nodes: [left: i32, right: i32, start: u32, len: u16]
- Vectors: [i16; 16] blocks (AVX2-aligned)
- Labels: [u8; 8] per block
- Reference indices: [u32; 8] per block
- Node class bits: [u8] for label deferral
```

## License

MIT
