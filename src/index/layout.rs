use crate::PACKED_DIMS;
use std::ptr;

// On-disk record sizes (written by `src/index/format.rs` via IndexWriter).
pub(crate) const PARTITION_STRIDE: usize = 80;
pub(crate) const NODE_STRIDE: usize = 80;

// Partition record layout (80 bytes):
// - u32 key (4) at offset 0
// - i32 root (4) at offset 4
// - i32 start (4) at offset 8 (unused at runtime)
// - i32 len (4) at offset 12 (unused at runtime)
// - i16 min[16] at offset 16
// - i16 max[16] at offset 48
const PART_KEY_OFF: usize = 0;
const PART_ROOT_OFF: usize = 4;
const PART_MIN_OFF: usize = 16;
const PART_MAX_OFF: usize = 48;

// Node record layout (80 bytes):
// - i32 left (4) at offset 0
// - i32 right (4) at offset 4
// - i32 start (4) at offset 8
// - i32 len (4) at offset 12
// - i16 min[16] at offset 16
// - i16 max[16] at offset 48
const NODE_LEFT_OFF: usize = 0;
const NODE_RIGHT_OFF: usize = 4;
const NODE_START_OFF: usize = 8;
const NODE_LEN_OFF: usize = 12;
const NODE_MIN_OFF: usize = 16;
const NODE_MAX_OFF: usize = 48;

#[inline(always)]
unsafe fn read_i32_unaligned(base: *const u8, off: usize) -> i32 {
    unsafe { ptr::read_unaligned(base.add(off).cast::<i32>()) }
}

#[inline(always)]
unsafe fn read_u32_unaligned(base: *const u8, off: usize) -> u32 {
    unsafe { ptr::read_unaligned(base.add(off).cast::<u32>()) }
}

#[inline(always)]
pub(crate) unsafe fn partition_key(partitions_base: *const u8, idx: usize) -> u32 {
    unsafe { read_u32_unaligned(partitions_base, idx * PARTITION_STRIDE + PART_KEY_OFF) }
}

#[inline(always)]
pub(crate) unsafe fn partition_root(partitions_base: *const u8, idx: usize) -> usize {
    unsafe { read_i32_unaligned(partitions_base, idx * PARTITION_STRIDE + PART_ROOT_OFF) as usize }
}

#[inline(always)]
pub(crate) unsafe fn partition_min<'a>(
    partitions_base: *const u8,
    idx: usize,
) -> &'a [i16; PACKED_DIMS] {
    // `min` starts at offset 16 inside the 80-byte record, and the first record starts at
    // file offset 46. Both ensure `i16` alignment (2 bytes), so returning a reference is OK.
    unsafe {
        &*partitions_base
            .add(idx * PARTITION_STRIDE + PART_MIN_OFF)
            .cast::<[i16; PACKED_DIMS]>()
    }
}

#[inline(always)]
pub(crate) unsafe fn partition_max<'a>(
    partitions_base: *const u8,
    idx: usize,
) -> &'a [i16; PACKED_DIMS] {
    unsafe {
        &*partitions_base
            .add(idx * PARTITION_STRIDE + PART_MAX_OFF)
            .cast::<[i16; PACKED_DIMS]>()
    }
}

#[inline(always)]
pub(crate) unsafe fn node_left(nodes_base: *const u8, idx: usize) -> i32 {
    unsafe { read_i32_unaligned(nodes_base, idx * NODE_STRIDE + NODE_LEFT_OFF) }
}

#[inline(always)]
pub(crate) unsafe fn node_right(nodes_base: *const u8, idx: usize) -> i32 {
    unsafe { read_i32_unaligned(nodes_base, idx * NODE_STRIDE + NODE_RIGHT_OFF) }
}

#[inline(always)]
pub(crate) unsafe fn node_start(nodes_base: *const u8, idx: usize) -> usize {
    unsafe { read_i32_unaligned(nodes_base, idx * NODE_STRIDE + NODE_START_OFF) as usize }
}

#[inline(always)]
pub(crate) unsafe fn node_len(nodes_base: *const u8, idx: usize) -> usize {
    unsafe { read_i32_unaligned(nodes_base, idx * NODE_STRIDE + NODE_LEN_OFF) as usize }
}

#[inline(always)]
pub(crate) unsafe fn node_min<'a>(nodes_base: *const u8, idx: usize) -> &'a [i16; PACKED_DIMS] {
    unsafe {
        &*nodes_base
            .add(idx * NODE_STRIDE + NODE_MIN_OFF)
            .cast::<[i16; PACKED_DIMS]>()
    }
}

#[inline(always)]
pub(crate) unsafe fn node_max<'a>(nodes_base: *const u8, idx: usize) -> &'a [i16; PACKED_DIMS] {
    unsafe {
        &*nodes_base
            .add(idx * NODE_STRIDE + NODE_MAX_OFF)
            .cast::<[i16; PACKED_DIMS]>()
    }
}
