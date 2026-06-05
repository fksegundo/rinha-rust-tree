use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub mod api;
pub mod fd_passing;
pub mod http;
pub mod index;
pub mod runtime;
pub mod vector;

pub const DIMS: usize = 14;
pub const PACKED_DIMS: usize = 16;
include!(concat!(env!("OUT_DIR"), "/scale.rs"));
pub const K: usize = 5;

pub type QueryVector = [i16; PACKED_DIMS];
