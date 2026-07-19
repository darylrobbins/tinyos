//! User address-space stub. Ring-3 support on x86_64 is a later milestone;
//! this keeps the arch-neutral object/scheduler code compiling. `new()`
//! returns None, so no user thread can ever be created on this arch.

#[derive(Clone, Copy, PartialEq)]
pub struct MapFlags {
    pub write: bool,
    pub exec: bool,
}

pub const USER_BASE: u64 = 0;
pub const APP_IMAGE_BASE: u64 = 0;

pub fn sync_icache(_pa: usize, _len: usize) {}
pub fn sync_dcache(_pa: usize, _len: usize) {}

pub struct AddrSpace;

impl AddrSpace {
    pub fn new() -> Option<Self> {
        None
    }

    pub fn ttbr1(&self) -> u64 {
        0
    }

    pub fn map(&mut self, _va: u64, _pa: usize, _len: usize, _f: MapFlags, _own: bool) -> Option<()> {
        None
    }

    pub fn map_page(&mut self, _va: u64, _pa: usize, _f: MapFlags) -> Option<()> {
        None
    }

    pub fn own_block(&mut self, _pa: usize, _pages: usize) {}

    pub fn protect(&mut self, _va: u64, _len: usize, _f: MapFlags) {}

    pub fn alloc_va(&mut self, _len: usize) -> u64 {
        0
    }

    pub fn user_buf_ok(&self, _va: u64, _len: u64, _write: bool) -> bool {
        false
    }
}
