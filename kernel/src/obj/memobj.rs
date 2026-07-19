//! Shared memory objects: contiguous frames, mappable into a process and
//! directly addressable by the kernel through the identity map (this is what
//! makes zero-copy window surfaces possible).

use alloc::sync::{Arc, Weak};

use abi::syscall::{ST_LIMIT_EXCEEDED, ST_NO_MEMORY};

use crate::mem::frames::{FRAME_SIZE, alloc_frames, free_frames};

use super::process::Process;

pub struct MemObj {
    base_pa: usize,
    pages: usize,
    size: usize,
    /// Quota accounting: the creating process, if user-created. Frames stay
    /// charged to the creator even if the handle is transferred.
    owner: Option<Weak<Process>>,
}

impl MemObj {
    /// Kernel-owned (unaccounted) memory object.
    pub fn create(size: usize) -> Option<Arc<Self>> {
        if size == 0 {
            return None;
        }
        let pages = size.div_ceil(FRAME_SIZE);
        let base_pa = alloc_frames(pages)?;
        Some(Arc::new(Self { base_pa, pages, size, owner: None }))
    }

    /// User-created memory object, charged against `owner`'s quota.
    pub fn create_for(size: usize, owner: &Arc<Process>) -> Result<Arc<Self>, u32> {
        if size == 0 {
            return Err(abi::syscall::ST_INVALID_ARGS);
        }
        let pages = size.div_ceil(FRAME_SIZE);
        if !owner.try_charge(pages * FRAME_SIZE) {
            return Err(ST_LIMIT_EXCEEDED);
        }
        let Some(base_pa) = alloc_frames(pages) else {
            owner.uncharge(pages * FRAME_SIZE);
            return Err(ST_NO_MEMORY);
        };
        Ok(Arc::new(Self {
            base_pa,
            pages,
            size,
            owner: Some(Arc::downgrade(owner)),
        }))
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
        if let Some(p) = self.owner.as_ref().and_then(Weak::upgrade) {
            p.uncharge(self.pages * FRAME_SIZE);
        }
    }
}
