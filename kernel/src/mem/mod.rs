pub mod frames;

use linked_list_allocator::LockedHeap;
use uefi::boot::MemoryType;
use uefi::mem::memory_map::{MemoryMap, MemoryMapOwned};

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

/// Claim the largest free conventional-memory region as the kernel heap.
/// Regions still in use (loader image, UEFI page tables, our stack — which
/// lives in BOOT_SERVICES_DATA) keep their own memory types, so CONVENTIONAL
/// entries are safe to take wholesale.
pub fn init_heap(map: &MemoryMapOwned) -> usize {
    let region = map
        .entries()
        .filter(|d| d.ty == MemoryType::CONVENTIONAL)
        .max_by_key(|d| d.page_count)
        .expect("no conventional memory");

    let size = region.page_count as usize * 4096;
    unsafe {
        HEAP.lock().init(region.phys_start as *mut u8, size);
    }
    size
}

/// (used, free) heap bytes.
pub fn stats() -> (usize, usize) {
    let heap = HEAP.lock();
    (heap.used(), heap.free())
}
