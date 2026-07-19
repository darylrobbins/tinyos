//! Filesystem facade: one lazily-mounted read-only FAT volume (the ESP).

pub mod fat;

use alloc::string::String;
use alloc::vec::Vec;

use spin::Mutex;

static FS: Mutex<Option<Option<fat::FatFs>>> = Mutex::new(None);

/// Run `f` with the mounted filesystem, mounting on first use.
/// Returns None if no volume could be mounted.
pub fn with<R>(f: impl FnOnce(&mut fat::FatFs) -> R) -> Option<R> {
    let mut g = FS.lock();
    let slot = g.get_or_insert_with(|| fat::FatFs::mount());
    slot.as_mut().map(f)
}

pub fn list(path: &str) -> Option<Vec<(String, usize, bool)>> {
    with(|fs| fs.list(path)).flatten()
}

pub fn read(path: &str) -> Option<Vec<u8>> {
    with(|fs| fs.read(path)).flatten()
}
