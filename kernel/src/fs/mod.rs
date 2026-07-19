//! Kernel side of tinyfs: virtio-blk adapter, the mounted-filesystem
//! singleton, and thin wrappers for the shell.
//!
//! All IO is synchronous polled virtio, so holding the FS spinlock across
//! an operation never crosses a scheduler yield/block point. If block IO
//! ever moves to WaitQueue-based completion, these wrappers must stop
//! holding a spin::Mutex for the duration.

use alloc::string::String;
use alloc::vec::Vec;

use tinyfs::{BlockDevice, DirEntryInfo, FsError, FsStats, Tinyfs, BLOCK_SIZE};

use crate::drivers::virtio_blk::{VirtioBlk, IO_SIZE};

/// tinyfs blocks are 4096 B = 8 virtio sectors.
const SECTORS_PER_BLOCK: u64 = (BLOCK_SIZE / 512) as u64;

pub struct BlkDev(VirtioBlk);

impl BlockDevice for BlkDev {
    fn block_count(&self) -> u64 {
        self.0.capacity_sectors() / SECTORS_PER_BLOCK
    }

    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        debug_assert_eq!(buf.len(), IO_SIZE);
        self.0
            .read(lba * SECTORS_PER_BLOCK, buf)
            .then_some(())
            .ok_or(FsError::Io)
    }

    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        debug_assert_eq!(buf.len(), IO_SIZE);
        self.0
            .write(lba * SECTORS_PER_BLOCK, buf)
            .then_some(())
            .ok_or(FsError::Io)
    }

    fn flush(&mut self) -> Result<(), FsError> {
        self.0.flush().then_some(()).ok_or(FsError::Io)
    }
}

static FS: spin::Mutex<Option<Tinyfs<BlkDev>>> = spin::Mutex::new(None);

/// Matches the terminal's fake wall clock: boot pretends it is
/// Fri Jul 17 2026, 9:41 am (Unix epoch 1784281260).
fn now_ms() -> u64 {
    1_784_281_260_000 + crate::arch::timer::uptime_ms()
}

/// Mount the filesystem if the disk carries one. Boot continues without a
/// filesystem on any failure; `mount` rejects non-tinyfs disks by magic, so
/// a foreign image is never written to.
pub fn init(blk: Option<VirtioBlk>) {
    let Some(blk) = blk else {
        kprintln!("tinyos: no block device, running diskless");
        return;
    };
    match Tinyfs::mount(BlkDev(blk)) {
        Ok(mut fs) => {
            fs.set_time_fn(now_ms);
            let st = fs.stats();
            kprintln!(
                "tinyos: tinyfs mounted, gen {}, {}/{} blocks used",
                st.generation,
                st.used_blocks,
                st.total_blocks
            );
            *FS.lock() = Some(fs);
        }
        Err(e) => kprintln!("tinyos: disk present but not mounted ({e})"),
    }
}

fn with_fs<R>(f: impl FnOnce(&mut Tinyfs<BlkDev>) -> Result<R, FsError>) -> Result<R, FsError> {
    match FS.lock().as_mut() {
        Some(fs) => f(fs),
        None => Err(FsError::NotMounted),
    }
}

pub fn read(cwd: &str, path: &str) -> Result<Vec<u8>, FsError> {
    with_fs(|fs| fs.read(cwd, path))
}

pub fn write(cwd: &str, path: &str, data: &[u8], append: bool) -> Result<(), FsError> {
    with_fs(|fs| fs.write(cwd, path, data, append))
}

pub fn list(cwd: &str, path: &str) -> Result<Vec<DirEntryInfo>, FsError> {
    with_fs(|fs| fs.list(cwd, path))
}

pub fn mkdir(cwd: &str, path: &str) -> Result<(), FsError> {
    with_fs(|fs| fs.mkdir(cwd, path))
}

pub fn remove(cwd: &str, path: &str, recursive: bool) -> Result<(), FsError> {
    with_fs(|fs| fs.remove(cwd, path, recursive))
}

pub fn rename(cwd: &str, from: &str, to: &str) -> Result<(), FsError> {
    with_fs(|fs| fs.rename(cwd, from, to))
}

pub fn stats() -> Result<FsStats, FsError> {
    with_fs(|fs| Ok(fs.stats()))
}

/// Canonicalize `path` against `cwd` and verify it is a directory
/// (for `cd`). Returns the canonical absolute path.
pub fn resolve_dir(cwd: &str, path: &str) -> Result<String, FsError> {
    with_fs(|fs| {
        let canon = tinyfs::path::canonical(cwd, path)?;
        let (_, inode) = fs.lookup("/", &canon)?;
        if inode.kind != tinyfs::InodeKind::Dir {
            return Err(FsError::NotADir);
        }
        Ok(canon)
    })
}
