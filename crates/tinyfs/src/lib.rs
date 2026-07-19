//! tinyfs — tinyOS's native copy-on-write filesystem.
//!
//! Shadow-paging CoW: no block is ever overwritten in place; each commit
//! writes a new checkpoint into the slot not currently live, so a crash at
//! any point leaves the previous checkpoint intact. No journal, no fsck,
//! no garbage collector — free space is rebuilt in RAM at mount by walking
//! the live checkpoint.
//!
//! The crate is `no_std` + `alloc` so the same code runs in the kernel and
//! in the host `mkfs-tinyfs` tool (and under `cargo test`).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod crc;
pub mod device;
pub mod layout;
pub mod path;

mod fs;

pub use device::{BlockDevice, MemDevice, BLOCK_SIZE};
pub use fs::{DirEntryInfo, FsStats, Tinyfs};
pub use layout::{FsError, InodeKind, MAX_FILE_SIZE, MAX_NAME, ROOT_INO};
