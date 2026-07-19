//! Physical frame façade. The kernel runs on UEFI's identity map, so any
//! heap allocation's VA is its PA — a "frame" is a 4 KiB-aligned zeroed heap
//! block returned by physical address. Callers must never assume this; the
//! phys-addr API lets a real bitmap allocator replace it without churn.

use alloc::alloc::{alloc_zeroed, dealloc};
use core::alloc::Layout;

pub const FRAME_SIZE: usize = 4096;

fn layout(n: usize) -> Layout {
    Layout::from_size_align(n * FRAME_SIZE, FRAME_SIZE).unwrap()
}

/// Allocate `n` contiguous zeroed frames; returns the physical base address.
pub fn alloc_frames(n: usize) -> Option<usize> {
    let p = unsafe { alloc_zeroed(layout(n)) };
    if p.is_null() { None } else { Some(p as usize) }
}

/// Free frames previously returned by `alloc_frames` with the same `n`.
///
/// # Safety
/// `pa` must come from `alloc_frames(n)` and must no longer be mapped or
/// referenced anywhere.
pub unsafe fn free_frames(pa: usize, n: usize) {
    unsafe { dealloc(pa as *mut u8, layout(n)) };
}
