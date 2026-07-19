//! Heap: one memobj mapped into our address space, handed to a linked-list
//! allocator. Lazily initialized by `entry::run` before main.

use linked_list_allocator::LockedHeap;

use crate::syscall::*;

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

const HEAP_SIZE: u64 = 1024 * 1024; // 1 MiB is plenty for demo apps

pub fn init() {
    let h = match syscall1(SYS_MEMOBJ_CREATE, HEAP_SIZE).ok() {
        Ok(h) => h,
        Err(_) => return, // no heap: allocations will fault, app should avoid them
    };
    if let Ok(va) = syscall3(SYS_MEMOBJ_MAP, h, 0, HEAP_SIZE).ok() {
        unsafe { HEAP.lock().init(va as *mut u8, HEAP_SIZE as usize) };
    }
}
