//! Physical frame allocator: a bitmap over every conventional UEFI region
//! the heap didn't claim. The kernel runs on UEFI's identity map, so frames
//! are addressable at their physical address; callers must still never
//! assume VA == PA outside this module's contract.
//!
//! If the pool runs dry (or was never initialized), allocation falls back to
//! 4 KiB-aligned zeroed heap blocks — `free_frames` routes each address back
//! to whichever allocator owns it.

use alloc::alloc::{alloc_zeroed, dealloc};
use alloc::vec::Vec;
use core::alloc::Layout;

use spin::Mutex;
use uefi::boot::MemoryType;
use uefi::mem::memory_map::{MemoryMap, MemoryMapOwned};

pub const FRAME_SIZE: usize = 4096;

struct Region {
    base: usize,
    frames: usize,
    free: usize,
    /// One bit per frame; set = allocated.
    bitmap: Vec<u64>,
}

static POOL: Mutex<Vec<Region>> = Mutex::new(Vec::new());

/// Adopt every conventional region except the heap's into the frame pool.
/// Call once at boot, after `init_heap` (the bitmaps live on the heap).
pub fn init(map: &MemoryMapOwned, heap_base: usize) -> usize {
    let mut pool = POOL.lock();
    let mut total = 0;
    for d in map
        .entries()
        .filter(|d| d.ty == MemoryType::CONVENTIONAL && d.phys_start as usize != heap_base)
    {
        let frames = d.page_count as usize;
        if frames == 0 {
            continue;
        }
        total += frames;
        pool.push(Region {
            base: d.phys_start as usize,
            frames,
            free: frames,
            bitmap: alloc::vec![0u64; frames.div_ceil(64)],
        });
    }
    total * FRAME_SIZE
}

/// (total, free) bytes in the frame pool (excludes the heap fallback).
pub fn pool_stats() -> (usize, usize) {
    let pool = POOL.lock();
    let (t, f) = pool
        .iter()
        .fold((0, 0), |(t, f), r| (t + r.frames, f + r.free));
    (t * FRAME_SIZE, f * FRAME_SIZE)
}

fn layout(n: usize) -> Layout {
    Layout::from_size_align(n * FRAME_SIZE, FRAME_SIZE).unwrap()
}

/// First-fit run of `n` clear bits, or None.
fn find_run(bitmap: &[u64], frames: usize, n: usize) -> Option<usize> {
    let (mut run, mut start) = (0usize, 0usize);
    for i in 0..frames {
        if bitmap[i / 64] >> (i % 64) & 1 == 1 {
            run = 0;
        } else {
            if run == 0 {
                start = i;
            }
            run += 1;
            if run == n {
                return Some(start);
            }
        }
    }
    None
}

/// Allocate `n` contiguous zeroed frames; returns the physical base address.
pub fn alloc_frames(n: usize) -> Option<usize> {
    debug_assert!(n > 0);
    {
        let mut pool = POOL.lock();
        for r in pool.iter_mut() {
            if r.free < n {
                continue;
            }
            let Some(start) = find_run(&r.bitmap, r.frames, n) else {
                continue;
            };
            for i in start..start + n {
                r.bitmap[i / 64] |= 1 << (i % 64);
            }
            r.free -= n;
            let pa = r.base + start * FRAME_SIZE;
            unsafe { core::ptr::write_bytes(pa as *mut u8, 0, n * FRAME_SIZE) };
            return Some(pa);
        }
    }
    // Pool dry or uninitialized: heap fallback.
    let p = unsafe { alloc_zeroed(layout(n)) };
    if p.is_null() { None } else { Some(p as usize) }
}

/// Free frames previously returned by `alloc_frames` with the same `n`.
///
/// # Safety
/// `pa` must come from `alloc_frames(n)` and must no longer be mapped or
/// referenced anywhere.
pub unsafe fn free_frames(pa: usize, n: usize) {
    let mut pool = POOL.lock();
    for r in pool.iter_mut() {
        if pa >= r.base && pa < r.base + r.frames * FRAME_SIZE {
            let start = (pa - r.base) / FRAME_SIZE;
            debug_assert!(start + n <= r.frames);
            for i in start..start + n {
                debug_assert!(r.bitmap[i / 64] >> (i % 64) & 1 == 1, "double free");
                r.bitmap[i / 64] &= !(1 << (i % 64));
            }
            r.free += n;
            return;
        }
    }
    drop(pool);
    unsafe { dealloc(pa as *mut u8, layout(n)) };
}
