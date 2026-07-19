//! On-disk format: constants, block layouts, and (de)serialization.
//!
//! All integers are little-endian. Layout:
//!
//! ```text
//! block 0      StaticHeader   written once by mkfs, immutable
//! block 1      Checkpoint A   commits alternate between the two slots;
//! block 2      Checkpoint B   mount picks the valid one with highest gen
//! block 3..    CoW-allocated  inode-table blocks, indirect blocks, data
//! ```

use crate::crc::crc32;

pub const BLOCK_SIZE: usize = 4096;

pub const MAGIC_HEADER: u64 = u64::from_le_bytes(*b"tinyfs\x01\0");
pub const MAGIC_CKPT: u64 = u64::from_le_bytes(*b"tfckpt\x01\0");
pub const VERSION: u16 = 1;

pub const INODE_COUNT: usize = 4096;
pub const INODE_SIZE: usize = 128;
pub const INODES_PER_BLOCK: usize = BLOCK_SIZE / INODE_SIZE; // 32
pub const ITAB_BLOCKS: usize = INODE_COUNT / INODES_PER_BLOCK; // 128

pub const DIRECT_PTRS: usize = 11;
pub const PTRS_PER_BLOCK: usize = BLOCK_SIZE / 8; // 512
/// 11 direct + 512 single-indirect + 512*512 double-indirect = 262,667.
pub const MAX_FILE_BLOCKS: usize = DIRECT_PTRS + PTRS_PER_BLOCK + PTRS_PER_BLOCK * PTRS_PER_BLOCK;
pub const MAX_FILE_SIZE: u64 = (MAX_FILE_BLOCKS * BLOCK_SIZE) as u64; // ~1 GiB

pub const DIRENT_SIZE: usize = 64;
pub const MAX_NAME: usize = 56;

pub const ROOT_INO: u32 = 1;

/// First block available to the CoW allocator.
pub const FIRST_DATA_BLOCK: u64 = 3;
/// Smallest disk worth formatting: metadata + a little room to breathe.
pub const MIN_BLOCKS: u64 = FIRST_DATA_BLOCK + ITAB_BLOCKS as u64 + 61; // 192 blocks = 768 KiB

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    Io,
    Corrupt,
    NoSpace,
    NoInodes,
    NotFound,
    Exists,
    NotADir,
    IsADir,
    NotEmpty,
    NameTooLong,
    FileTooBig,
    InvalidPath,
    NotMounted,
}

impl FsError {
    pub fn as_str(&self) -> &'static str {
        match self {
            FsError::Io => "i/o error",
            FsError::Corrupt => "filesystem corrupt",
            FsError::NoSpace => "no space left on device",
            FsError::NoInodes => "out of inodes",
            FsError::NotFound => "no such file or directory",
            FsError::Exists => "file exists",
            FsError::NotADir => "not a directory",
            FsError::IsADir => "is a directory",
            FsError::NotEmpty => "directory not empty",
            FsError::NameTooLong => "name too long",
            FsError::FileTooBig => "file too big",
            FsError::InvalidPath => "invalid path",
            FsError::NotMounted => "no filesystem mounted",
        }
    }
}

impl core::fmt::Display for FsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeKind {
    Free,
    File,
    Dir,
}

impl InodeKind {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(InodeKind::Free),
            1 => Some(InodeKind::File),
            2 => Some(InodeKind::Dir),
            _ => None,
        }
    }

    pub fn as_u16(self) -> u16 {
        match self {
            InodeKind::Free => 0,
            InodeKind::File => 1,
            InodeKind::Dir => 2,
        }
    }
}

// -- little-endian field helpers ---------------------------------------------

fn r16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn r32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}
fn r64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}
fn w16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn w32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn w64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

// -- static header (block 0) --------------------------------------------------
// magic u64 @0, version u16 @8, block_size u32 @12, total_blocks u64 @16,
// inode_count u32 @24, crc32 u32 @28. CRC over the whole block with the
// crc field zeroed.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticHeader {
    pub total_blocks: u64,
}

impl StaticHeader {
    pub fn encode(&self, block: &mut [u8]) {
        assert_eq!(block.len(), BLOCK_SIZE);
        block.fill(0);
        w64(block, 0, MAGIC_HEADER);
        w16(block, 8, VERSION);
        w32(block, 12, BLOCK_SIZE as u32);
        w64(block, 16, self.total_blocks);
        w32(block, 24, INODE_COUNT as u32);
        let crc = crc32(block);
        w32(block, 28, crc);
    }

    pub fn decode(block: &[u8]) -> Result<Self, FsError> {
        if block.len() != BLOCK_SIZE || r64(block, 0) != MAGIC_HEADER {
            return Err(FsError::Corrupt);
        }
        let mut copy = [0u8; BLOCK_SIZE];
        copy.copy_from_slice(block);
        let stored = r32(&copy, 28);
        w32(&mut copy, 28, 0);
        if crc32(&copy) != stored {
            return Err(FsError::Corrupt);
        }
        if r16(block, 8) != VERSION
            || r32(block, 12) != BLOCK_SIZE as u32
            || r32(block, 24) != INODE_COUNT as u32
        {
            return Err(FsError::Corrupt);
        }
        Ok(Self {
            total_blocks: r64(block, 16),
        })
    }
}

// -- checkpoint (blocks 1 and 2) ----------------------------------------------
// magic u64 @0, generation u64 @8, used_blocks u64 @16, itab [u64;128] @24,
// crc32 u32 @4092. CRC over bytes 0..4092.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub generation: u64,
    pub used_blocks: u64,
    pub itab: [u64; ITAB_BLOCKS],
}

impl Checkpoint {
    /// Which checkpoint block a given generation lives in (gen 1 -> block 1).
    pub fn slot_for(generation: u64) -> u64 {
        1 + ((generation + 1) % 2)
    }

    pub fn encode(&self, block: &mut [u8]) {
        assert_eq!(block.len(), BLOCK_SIZE);
        block.fill(0);
        w64(block, 0, MAGIC_CKPT);
        w64(block, 8, self.generation);
        w64(block, 16, self.used_blocks);
        for (i, &b) in self.itab.iter().enumerate() {
            w64(block, 24 + i * 8, b);
        }
        let crc = crc32(&block[..BLOCK_SIZE - 4]);
        w32(block, BLOCK_SIZE - 4, crc);
    }

    pub fn decode(block: &[u8]) -> Result<Self, FsError> {
        if block.len() != BLOCK_SIZE || r64(block, 0) != MAGIC_CKPT {
            return Err(FsError::Corrupt);
        }
        if crc32(&block[..BLOCK_SIZE - 4]) != r32(block, BLOCK_SIZE - 4) {
            return Err(FsError::Corrupt);
        }
        let mut itab = [0u64; ITAB_BLOCKS];
        for (i, slot) in itab.iter_mut().enumerate() {
            *slot = r64(block, 24 + i * 8);
        }
        Ok(Self {
            generation: r64(block, 8),
            used_blocks: r64(block, 16),
            itab,
        })
    }
}

// -- inode (128 bytes, 32 per table block) -------------------------------------
// kind u16 @0, flags u16 @2, size u64 @8, mtime_ms u64 @16,
// direct [u64;11] @24, indirect u64 @112, dindirect u64 @120.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inode {
    pub kind: InodeKind,
    pub size: u64,
    pub mtime_ms: u64,
    pub direct: [u64; DIRECT_PTRS],
    pub indirect: u64,
    /// Block of 512 pointers to blocks of 512 data pointers.
    pub dindirect: u64,
}

impl Inode {
    pub const fn empty() -> Self {
        Self {
            kind: InodeKind::Free,
            size: 0,
            mtime_ms: 0,
            direct: [0; DIRECT_PTRS],
            indirect: 0,
            dindirect: 0,
        }
    }

    pub fn encode(&self, buf: &mut [u8]) {
        assert_eq!(buf.len(), INODE_SIZE);
        buf.fill(0);
        w16(buf, 0, self.kind.as_u16());
        w64(buf, 8, self.size);
        w64(buf, 16, self.mtime_ms);
        for (i, &b) in self.direct.iter().enumerate() {
            w64(buf, 24 + i * 8, b);
        }
        w64(buf, 112, self.indirect);
        w64(buf, 120, self.dindirect);
    }

    pub fn decode(buf: &[u8]) -> Result<Self, FsError> {
        assert_eq!(buf.len(), INODE_SIZE);
        let kind = InodeKind::from_u16(r16(buf, 0)).ok_or(FsError::Corrupt)?;
        let mut direct = [0u64; DIRECT_PTRS];
        for (i, slot) in direct.iter_mut().enumerate() {
            *slot = r64(buf, 24 + i * 8);
        }
        Ok(Self {
            kind,
            size: r64(buf, 8),
            mtime_ms: r64(buf, 16),
            direct,
            indirect: r64(buf, 112),
            dindirect: r64(buf, 120),
        })
    }
}

// -- directory entry (64 bytes, packed as directory file content) --------------
// ino u32 @0, kind u8 @4, name_len u8 @5, name [u8;56] @8. ino 0 = empty slot
// (never written; directories are compacted on remove).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirent {
    pub ino: u32,
    pub kind: InodeKind,
    pub name: [u8; MAX_NAME],
    pub name_len: u8,
}

impl Dirent {
    pub fn new(ino: u32, kind: InodeKind, name: &str) -> Result<Self, FsError> {
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_NAME {
            return Err(FsError::NameTooLong);
        }
        let mut buf = [0u8; MAX_NAME];
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            ino,
            kind,
            name: buf,
            name_len: bytes.len() as u8,
        })
    }

    pub fn name_str(&self) -> Result<&str, FsError> {
        core::str::from_utf8(&self.name[..self.name_len as usize]).map_err(|_| FsError::Corrupt)
    }

    pub fn encode(&self, buf: &mut [u8]) {
        assert_eq!(buf.len(), DIRENT_SIZE);
        buf.fill(0);
        w32(buf, 0, self.ino);
        buf[4] = self.kind.as_u16() as u8;
        buf[5] = self.name_len;
        buf[8..8 + MAX_NAME].copy_from_slice(&self.name);
    }

    pub fn decode(buf: &[u8]) -> Result<Self, FsError> {
        assert_eq!(buf.len(), DIRENT_SIZE);
        let kind = InodeKind::from_u16(buf[4] as u16).ok_or(FsError::Corrupt)?;
        let name_len = buf[5];
        if name_len as usize > MAX_NAME {
            return Err(FsError::Corrupt);
        }
        let mut name = [0u8; MAX_NAME];
        name.copy_from_slice(&buf[8..8 + MAX_NAME]);
        Ok(Self {
            ino: r32(buf, 0),
            kind,
            name,
            name_len,
        })
    }
}
