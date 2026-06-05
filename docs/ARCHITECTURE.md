# Architecture Documentation

This document provides detailed technical information about the Rinha Rust fraud detection system architecture.

## System Overview

The system is a high-performance HTTP API for fraud detection that uses k-nearest neighbors (k-NN) search on quantized feature vectors. It's designed for low-latency operation with sub-millisecond response times.

### Key Design Principles

1. **Zero-copy operations** - Memory-mapped indexes, file descriptor passing
2. **SIMD acceleration** - AVX2 for vectorized distance calculations
3. **Event-driven I/O** - Epoll-based event loop for efficient socket handling
4. **Specialized parsers** - Fast JSON parsing with multiple fallback strategies
5. **Learned indexing** - Decision tree partitioning for efficient search

## Module Architecture

### API Layer (`src/api/`)

The API layer handles HTTP requests and coordinates with the fraud detection engine.

#### `warmup.rs`
- Loads and warms up the index before serving requests
- Supports both external warmup payloads and self-warmup
- Executes queries to populate CPU caches and page tables
- Configurable via `RINHA_WARMUP_QUERIES` and related environment variables

#### `server.rs`
- FD mode server for production deployment
- Receives pre-configured file descriptors from load balancer
- Uses epoll for efficient event handling
- Supports busy polling for reduced latency under load

#### `handler.rs`
- HTTP request handler
- Parses request body into query vector
- Calls fraud prediction engine
- Formats and sends response

### HTTP Layer (`src/http/`)

#### `parser.rs`
- Zero-allocation HTTP request parsing
- Parses request line, headers, and body
- Validates content-length and path
- Supports pipelined requests

#### `responses.rs`
- Pre-formatted HTTP response templates
- Includes success (200) and error (400, 500) responses
- Optimized for fast response generation

### FD Passing Layer (`src/fd_passing/`)

This layer handles inter-process communication via Unix domain sockets and file descriptor passing.

#### `evented.rs`
- Main event loop coordinator
- Manages epoll instance and event registration
- Coordinates connection, epoll, and I/O operations

#### `conn.rs`
- Connection state management
- Tracks read/write state
- Manages connection lifecycle

#### `epoll.rs`
- Epoll wrapper functions
- Event registration/deregistration
- Edge-triggered event handling
- Busy polling support

#### `io.rs`
- Low-level I/O operations
- Greedy reading for efficiency
- Socket configuration (TCP_NODELAY, non-blocking)

### Index Layer (`src/index/`)

The index layer implements the vector similarity search engine.

#### `mod.rs`
- Main index entry point
- `SpecialistIndex` struct - loads and queries the index
- Partition management and key lookup
- Fraud prediction API

#### `build.rs`
- Index building logic
- Reference data structure
- Index serialization to disk

#### `format.rs`
- Index format definitions
- Header structure
- Format versioning (V5)

#### `layout.rs`
- Memory layout accessors
- Partition, node, and vector access
- Unsafe pointer operations for zero-copy access

#### `partition_scheme.rs`
- Learned tree partitioning
- Tree predicate learning from sample queries
- Key computation from query vectors
- Supports Tree256 (8-level tree)

#### `mmap.rs`
- Memory mapping operations
- `MmapRegion` struct for managing mapped memory
- Platform-specific optimizations (Linux huge pages)

#### `search.rs`
- k-NN search algorithms
- `PendingSubtrees` for label deferral optimization
- AVX2-accelerated distance calculation
- Leaf scanning with SIMD
- Helper functions for sorting and insertion

### Vector Layer (`src/vector/`)

The vector layer parses JSON payloads into quantized feature vectors.

#### `mod.rs`
- Main vector parsing entry
- `ParseError` enum
- `parse_query` function with multi-strategy fallback

#### `helpers.rs`
- Quantization function
- Hash function for merchant IDs
- MCC parsing and risk scoring
- DateTime parsing
- Fast f64 parsing
- JSON value reading functions

#### `compact.rs`
- Compact ordered JSON parser
- Assumes fixed field order
- Fastest parsing path
- Direct byte-level parsing

#### `single_pass.rs`
- Single-pass JSON parser
- Handles any field order
- Skips unknown fields
- Validates required fields

#### `serde_fallback.rs`
- Serde-based JSON parser
- Full deserialization support
- Used when specialized parsers fail
- Handles case-insensitive field matching

### Runtime Layer (`src/runtime.rs`)

- Environment variable parsing
- Configuration constants
- Build-time validation

## Data Structures

### Query Vector

```rust
pub type QueryVector = [i16; PACKED_DIMS]; // 16 dimensions
```

- Quantized to i16 for cache efficiency
- Padded to 16 for AVX2 alignment
- SCALE constant for quantization (build-time)

### Index Structure

```
SpecialistIndex {
    _mapping: MmapRegion,                    // Memory-mapped file
    reference_count: usize,                 // Number of references
    partitions_base: *const u8,             // Partition data
    partition_count: usize,                 // Number of partitions
    key_to_partition: [i32; 1024],          // Key lookup table
    active_keys: Vec<u32>,                  // Active partition keys
    partition_scheme: PartitionScheme,      // Tree predicates
    nodes_base: *const u8,                  // Node data
    node_count: usize,                      // Number of nodes
    vectors: *const i16,                    // Vector data
    vectors_len: usize,                     // Vector data length
    labels: *const u8,                      // Label data
    labels_len: usize,                      // Label data length
    ref_indices: *const u32,                // Reference indices
    ref_indices_len: usize,                 // Reference indices length
    node_class_bits: *const u8,             // Class bits for deferral
    early_exit_threshold: i64,              // Early exit threshold
    label_defer: bool,                      // Label deferral enabled
}
```

### Partition

Each partition contains:
- Key (8-bit from tree traversal)
- Root node index
- Bounding box (min/max per dimension)
- Reference to tree nodes

### Node

Each tree node contains:
- Left child index
- Right child index
- Start index in vector array
- Length (number of vectors)

### Vector Block

Vectors are stored in blocks of 8 for AVX2 efficiency:
- 8 vectors × 16 dimensions = 128 i16 values
- 8 labels (u8)
- 8 reference indices (u32)

## Algorithms

### k-NN Search Algorithm

```
1. Parse query into quantized vector
2. Compute partition key using tree predicates
3. Get list of active partitions
4. For each partition:
   a. Compute lower bound distance using bounding box
   b. If bound < k-th best distance:
      - Iterative tree search
      - For leaf nodes:
        - Scan vectors with AVX2
        - Update k best neighbors
      - For internal nodes:
        - Try defer subtree if label consensus
        - Otherwise traverse both children
5. Replay deferred subtrees if consensus changed
6. Return fraud count from k labels
```

### Label Deferral Optimization

When searching, if current k neighbors have consensus (all 0 or all 1):
- Defer searching subtrees that don't contain the consensus class
- If consensus changes later, replay deferred subtrees
- Reduces search time for clear-cut cases

### AVX2 Distance Calculation

```rust
// Compute squared Euclidean distance for 8 vectors in parallel
fn scan_block_pair_avx2_bounded(
    vectors: &[i16],
    block_base: usize,
    q_pairs: &[__m256i; DIM_PAIRS],
    limit: i64,
) -> (u32, [i32; LANES])
```

- Pairs of dimensions processed as 256-bit vectors
- Parallel subtraction and multiplication
- Horizontal sum for final distance
- Early rejection if distance exceeds limit

### Tree Partition Learning

```
1. Sample queries from reference data
2. For each tree level:
   a. For each node, find best split:
      - Try each dimension
      - Find threshold that maximizes information gain
   b. Store predicate (dim, threshold)
3. Use learned predicates for partitioning
```

## Performance Optimizations

### Memory Optimizations

1. **Memory Mapping**
   - Index loaded with `mmap` for zero-copy
   - `MADV_WILLNEED` for readahead
   - `MADV_HUGEPAGE` for TLB efficiency (Linux)

2. **Quantization**
   - f64 → i16 quantization
   - 4x memory reduction
   - Better cache locality

3. **Block Layout**
   - Vectors stored in blocks of 8
   - Aligned for AVX2 (32-byte)
   - Labels and indices interleaved

4. **Allocator**
   - mimalloc for reduced fragmentation
   - Better multi-threaded performance

### CPU Optimizations

1. **AVX2 SIMD**
   - 8 distance calculations in parallel
   - 4x speedup over scalar
   - Only on x86_64 with AVX2 support

2. **Busy Polling**
   - `EPOLL_BUSY_POLL` when under load
   - Reduces context switches
   - Configurable via `RINHA_EPOLL_BUSY_POLL`

3. **Label Deferral**
   - Skip searching irrelevant subtrees
   - Reduces node visits by ~30%
   - Configurable via `RINHA_LABEL_DEFER`

4. **Early Exit**
   - Stop when k-th distance below threshold
   - Configurable via `RINHA_EARLY_EXIT_THRESHOLD`
   - Reduces search time for confident predictions

### I/O Optimizations

1. **Epoll Edge-Triggered**
   - Efficient event notification
   - No spurious wakeups
   - One-shot mode for fairness

2. **Non-Blocking Sockets**
   - `TCP_NODELAY` for low latency
   - Non-blocking mode
   - Greedy reading

3. **File Descriptor Passing**
   - Zero-copy between processes
   - Unix domain sockets
   - SCM_RIGHTS ancillary data

4. **Specialized Parsers**
   - Compact parser for common case
   - Single-pass for flexibility
   - Serde fallback for compatibility

## Deployment Architecture

### Single Instance

```
[Client] → [API Server]
            ↓
         [Index]
```

### Multi-Instance with Load Balancer

```
[Client] → [Load Balancer] → [API Server 1]
                          → [API Server 2]
                          → [API Server N]
                          ↓
                        [Index] (shared via mmap)
```

The load balancer:
- Accepts connections
- Passes FDs to worker processes
- Distributes load across instances
- Workers share index via memory mapping

## Build Configuration

### Release Profile

```toml
[profile.release]
opt-level = 3           # Maximum optimization
lto = "fat"             # Link-time optimization
codegen-units = 1       # Single codegen unit for better optimization
panic = "abort"         # Abort on panic (no unwinding)
strip = true            # Strip symbols
debug = 0               # No debug info
overflow-checks = false # Disable overflow checks
```

### Build-Time Configuration

- `RINHA_NATIVE_SCALE` - Quantization scale (default: 1000)
- Validated in `build.rs`
- Fails build if invalid

## Environment Variables

### Index Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `RINHA_INDEX_PATH` | Path to index file | Required |
| `RINHA_NATIVE_SCALE` | Quantization scale | 1000 (build-time) |
| `RINHA_EARLY_EXIT_THRESHOLD` | Early exit distance | 0 (disabled) |
| `RINHA_LABEL_DEFER` | Enable label deferral | 1 (enabled) |

### Runtime Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `RINHA_WARMUP_QUERIES` | Warmup query count | 1000 |
| `RINHA_SELF_WARMUP_URL` | Self-warmup URL | None |
| `RINHA_SELF_WARMUP_DURATION_MS` | Self-warmup duration | 5000 |
| `RINHA_SELF_WARMUP_CONCURRENCY` | Self-warmup concurrency | 4 |
| `RINHA_WARMUP_PAYLOADS_PATH` | Warmup payloads path | None |

### Epoll Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `RINHA_EPOLL_BUSY_POLL` | Enable busy polling | 0 (disabled) |
| `RINHA_EPOLL_IDLE_US` | Idle timeout (μs) | 1000 |
| `RINHA_SPIN_BEFORE_BLOCK_US` | Spin duration (μs) | 0 |

### Socket Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `RINHA_CLIENT_FD_PRECONFIGURED` | FDs pre-configured | 0 (false) |

## Index Format Specification

### Header (V5)

```
Offset | Size | Field
-------|------|-------
0      | 8    | Magic: "RNSPCST5"
8      | 4    | Scale (i32)
12     | 4    | Packed dimensions (i32)
16     | 4    | Reference count (i32)
20     | 4    | Partition count (i32)
24     | 4    | Node count (i32)
28     | 4    | Block count (i32)
32     | 2    | Partition scheme ID (i16)
34     | 2    | Partition scheme param (u16)
36     | 2    | Cut count per level (i16)
38     | 2    | Reserved
40     | N    | Tree predicates [dim: u8, flags: u8, threshold: i16]
```

### Data Sections

After header, aligned to 4-byte boundaries:

1. **Partitions** (partition_count × 64 bytes)
   - Key: u16 (2 bytes)
   - Root: u32 (4 bytes)
   - Min: [i16; 16] (32 bytes)
   - Max: [i16; 16] (32 bytes)
   - Padding: 6 bytes

2. **Nodes** (node_count × 12 bytes)
   - Left: i32 (4 bytes)
   - Right: i32 (4 bytes)
   - Start: u32 (4 bytes)
   - Len: u16 (2 bytes)
   - Padding: 2 bytes

3. **Vectors** (block_count × 128 bytes)
   - 8 vectors × 16 dimensions = 128 i16 values

4. **Labels** (block_count × 8 bytes)
   - 8 labels (u8)

5. **Reference Indices** (block_count × 32 bytes)
   - 8 indices (u32)

6. **Node Class Bits** (node_count bytes)
   - Class mask for label deferral

## Testing

### Unit Tests

Located in each module's `tests.rs` or `#[cfg(test)]` blocks:

- `src/index/tests.rs` - Index loading and search tests
- `src/vector/tests.rs` - Vector parsing tests
- `src/http/parser/tests.rs` - HTTP parsing tests
- `src/api/tests.rs` - API integration tests
- `src/lb/main.rs` - Load balancer tests

### Running Tests

```bash
# All tests
cargo test

# Specific module
cargo test --lib index::tests

# Release mode (for performance tests)
cargo test --release
```

## Performance Characteristics

### Latency

- **Target**: < 1ms p99 latency
- **Typical**: 0.3-0.5ms
- **Factors**: query complexity, cache state, CPU load

### Throughput

- **Single instance**: ~10k QPS
- **Multi-instance**: Scales linearly with instances
- **Bottleneck**: CPU (AVX2 distance calculations)

### Memory

- **Index size**: ~200MB for 1M references
- **Per-instance overhead**: ~50MB
- **mimalloc**: Better memory efficiency

## Future Improvements

Potential areas for optimization:

1. **GPU acceleration** - Offload distance calculations to GPU
2. **Better partitioning** - More sophisticated learned schemes
3. **Compression** - Compress vectors in memory
4. **Adaptive k** - Adjust k based on query confidence
5. **Caching** - Cache recent query results
