//! Block device abstraction. The kernel backs this with virtio-blk, the
//! host tool with a file, tests with `MemDevice`.

use alloc::vec;
use alloc::vec::Vec;

use crate::layout::FsError;

pub const BLOCK_SIZE: usize = crate::layout::BLOCK_SIZE;

pub trait BlockDevice {
    fn block_count(&self) -> u64;
    /// `buf` must be exactly `BLOCK_SIZE` bytes.
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError>;
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError>;
    /// Barrier: all previous writes are durable before any later write.
    fn flush(&mut self) -> Result<(), FsError>;
}

/// In-memory device for tests and scratch use.
#[derive(Clone)]
pub struct MemDevice {
    data: Vec<u8>,
}

impl MemDevice {
    pub fn new(blocks: u64) -> Self {
        Self {
            data: vec![0; blocks as usize * BLOCK_SIZE],
        }
    }

    pub fn from_bytes(data: Vec<u8>) -> Self {
        assert_eq!(data.len() % BLOCK_SIZE, 0);
        Self { data }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
}

impl BlockDevice for MemDevice {
    fn block_count(&self) -> u64 {
        (self.data.len() / BLOCK_SIZE) as u64
    }

    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let off = lba as usize * BLOCK_SIZE;
        if buf.len() != BLOCK_SIZE || off + BLOCK_SIZE > self.data.len() {
            return Err(FsError::Io);
        }
        buf.copy_from_slice(&self.data[off..off + BLOCK_SIZE]);
        Ok(())
    }

    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        let off = lba as usize * BLOCK_SIZE;
        if buf.len() != BLOCK_SIZE || off + BLOCK_SIZE > self.data.len() {
            return Err(FsError::Io);
        }
        self.data[off..off + BLOCK_SIZE].copy_from_slice(buf);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}
