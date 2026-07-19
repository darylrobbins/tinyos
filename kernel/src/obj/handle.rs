//! Per-process handle tables: handle value = slot index + 1 (0 is invalid).
//! Rights only ever narrow (dup with a mask); TRANSFER moves a handle out of
//! the table and onto a channel message.

use alloc::vec::Vec;

use super::Object;
use super::syscall::{ST_ACCESS_DENIED, ST_BAD_HANDLE, ST_LIMIT_EXCEEDED};

pub const RIGHT_READ: u32 = 1;
pub const RIGHT_WRITE: u32 = 2;
pub const RIGHT_DUP: u32 = 4;
pub const RIGHT_TRANSFER: u32 = 8;
pub const RIGHT_MAP: u32 = 16;
pub const RIGHT_WAIT: u32 = 32;
pub const RIGHTS_ALL: u32 = 0x3F;

const MAX_HANDLES: usize = 256;

#[derive(Clone)]
pub struct Handle {
    pub object: Object,
    pub rights: u32,
}

impl Handle {
    pub fn new(object: Object, rights: u32) -> Self {
        Self { object, rights }
    }
}

pub struct HandleTable {
    slots: Vec<Option<Handle>>,
}

impl HandleTable {
    pub const fn new() -> Self {
        Self { slots: Vec::new() }
    }

    pub fn insert(&mut self, h: Handle) -> Result<u32, u32> {
        if let Some(i) = self.slots.iter().position(Option::is_none) {
            self.slots[i] = Some(h);
            return Ok(i as u32 + 1);
        }
        if self.slots.len() >= MAX_HANDLES {
            return Err(ST_LIMIT_EXCEEDED);
        }
        self.slots.push(Some(h));
        Ok(self.slots.len() as u32)
    }

    pub fn get(&self, hv: u32) -> Result<&Handle, u32> {
        self.slots
            .get(hv.wrapping_sub(1) as usize)
            .and_then(Option::as_ref)
            .ok_or(ST_BAD_HANDLE)
    }

    /// Remove and return (close / transfer).
    pub fn take(&mut self, hv: u32) -> Result<Handle, u32> {
        self.slots
            .get_mut(hv.wrapping_sub(1) as usize)
            .and_then(Option::take)
            .ok_or(ST_BAD_HANDLE)
    }

    /// Reinstate a handle at its old (just-freed) slot — the undo path for
    /// a failed transfer. Returns false if the slot was re-taken meanwhile.
    pub fn insert_back(&mut self, hv: u32, h: Handle) -> bool {
        match self.slots.get_mut(hv.wrapping_sub(1) as usize) {
            Some(slot @ None) => {
                *slot = Some(h);
                true
            }
            _ => false,
        }
    }

    /// Duplicate with narrowed rights: `mask` is ANDed in, never widens.
    pub fn dup(&mut self, hv: u32, mask: u32) -> Result<u32, u32> {
        let h = self.get(hv)?;
        if h.rights & RIGHT_DUP == 0 {
            return Err(ST_ACCESS_DENIED);
        }
        let new = Handle::new(h.object.clone(), h.rights & mask);
        self.insert(new)
    }

    /// Drop every handle (process teardown); peers observe closes.
    pub fn clear(&mut self) {
        self.slots.clear();
    }
}
