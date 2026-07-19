//! Shared memory objects: contiguous frames, mappable into a process and
//! directly addressable by the kernel through the identity map (this is what
//! makes zero-copy window surfaces possible).

use alloc::sync::Arc;

use crate::mem::frames::{FRAME_SIZE, alloc_frames, free_frames};

pub struct MemObj {
    base_pa: usize,
    pages: usize,
    size: usize,
}

impl MemObj {
    pub fn create(size: usize) -> Option<Arc<Self>> {
        if size == 0 {
            return None;
        }
        let pages = size.div_ceil(FRAME_SIZE);
        let base_pa = alloc_frames(pages)?;
        Some(Arc::new(Self { base_pa, pages, size }))
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn pa(&self) -> usize {
        self.base_pa
    }

    /// Kernel-side view of the backing memory (identity-mapped).
    ///
    /// # Safety
    /// Racy by design — userspace writes the same bytes. Callers must treat
    /// contents as untrusted and tolerate concurrent modification.
    pub unsafe fn bytes(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.base_pa as *mut u8, self.pages * FRAME_SIZE) }
    }
}

impl Drop for MemObj {
    fn drop(&mut self) {
        // All mappings die with their address spaces before the last Arc
        // can drop (Process holds the MemObj Arcs it mapped).
        unsafe { free_frames(self.base_pa, self.pages) };
    }
}
