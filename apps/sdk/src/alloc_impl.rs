//! Heap: one memobj mapped into our address space, handed to a linked-list
//! allocator. Lazily initialized by `entry::run` before main.

use linked_list_allocator::LockedHeap;

use crate::syscall::*;

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

// Sized for GUI apps that keep a window-sized back buffer (a 920x640 BGRA
// frame alone is ~2.3 MiB) plus game state.
const HEAP_SIZE: u64 = 8 * 1024 * 1024;

pub fn init() {
    let h = match syscall1(SYS_MEMOBJ_CREATE, HEAP_SIZE).ok() {
        Ok(h) => h,
        Err(_) => return, // no heap: allocations will fault, app should avoid them
    };
    if let Ok(va) = syscall3(SYS_MEMOBJ_MAP, h, 0, HEAP_SIZE).ok() {
        unsafe { HEAP.lock().init(va as *mut u8, HEAP_SIZE as usize) };
    }
}
