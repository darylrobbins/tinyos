//! Crash consistency: cut the write stream at every possible point during a
//! mutation and prove that a remount always sees either the old tree or the
//! new tree, never corruption.
//!
//! Model: writes are applied in submission order (the device does not
//! reorder across our flush barriers; tinyfs only relies on ordering at its
//! two flush points, and QEMU/virtio preserve per-request completion we wait
//! on synchronously).

use tinyfs::{BlockDevice, FsError, MemDevice, Tinyfs, BLOCK_SIZE};

/// Records every write so the image can be replayed up to a cut point.
struct RecDev {
    inner: MemDevice,
    log: Vec<(u64, Vec<u8>)>,
}

impl RecDev {
    fn new(inner: MemDevice) -> Self {
        Self {
            inner,
            log: Vec::new(),
        }
    }
}

impl BlockDevice for RecDev {
    fn block_count(&self) -> u64 {
        self.inner.block_count()
    }
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.inner.read_block(lba, buf)
    }
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        self.log.push((lba, buf.to_vec()));
        self.inner.write_block(lba, buf)
    }
    fn flush(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}

fn tree_snapshot(fs: &mut Tinyfs<MemDevice>) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![String::from("/")];
    while let Some(dir) = stack.pop() {
        for e in fs.list("/", &dir).unwrap() {
            let path = if dir == "/" {
                format!("/{}", e.name)
            } else {
                format!("{}/{}", dir, e.name)
            };
            match e.kind {
                tinyfs::InodeKind::Dir => stack.push(path),
                _ => out.push((path.clone(), fs.read("/", &path).unwrap())),
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_write_prefix_mounts_as_old_or_new() {
    // Build the "old" state.
    let mut fs = Tinyfs::format(MemDevice::new(1024)).unwrap();
    fs.mkdir("/", "/docs").unwrap();
    fs.write("/", "/docs/keep.txt", b"keep me", false).unwrap();
    fs.write("/", "/victim.txt", b"old contents", false).unwrap();
    let img_old = fs.into_inner();

    let mut old_fs = Tinyfs::mount(img_old.clone()).unwrap();
    let old_tree = tree_snapshot(&mut old_fs);

    // Perform a compound mutation through a recording device.
    let mut fs = Tinyfs::mount(RecDev::new(img_old.clone())).unwrap();
    fs.write("/", "/victim.txt", b"NEW contents, longer than before", false)
        .unwrap();
    let rec = fs.into_inner();
    let full_log = rec.log;

    let mut fs = Tinyfs::mount(rec.inner).unwrap();
    let new_tree = tree_snapshot(&mut fs);
    assert_ne!(old_tree, new_tree);

    // Cut the write stream at every prefix and remount.
    let mut saw_old = 0;
    let mut saw_new = 0;
    for cut in 0..=full_log.len() {
        let mut dev = img_old.clone();
        for (lba, data) in &full_log[..cut] {
            dev.write_block(*lba, data).unwrap();
        }
        let mut fs = Tinyfs::mount(dev)
            .unwrap_or_else(|e| panic!("mount failed at cut {cut}/{}: {e:?}", full_log.len()));
        fs.check()
            .unwrap_or_else(|e| panic!("check failed at cut {cut}: {e:?}"));
        let tree = tree_snapshot(&mut fs);
        if tree == old_tree {
            saw_old += 1;
        } else if tree == new_tree {
            saw_new += 1;
        } else {
            panic!("cut {cut}: tree is neither old nor new: {tree:?}");
        }
    }
    // The transition must actually happen, exactly once, at the checkpoint write.
    assert!(saw_old >= 1 && saw_new >= 1);
    assert_eq!(saw_old + saw_new, full_log.len() + 1);
}

#[test]
fn torn_checkpoint_block_falls_back_to_previous() {
    let mut fs = Tinyfs::format(MemDevice::new(1024)).unwrap();
    fs.write("/", "/a.txt", b"gen2 state", false).unwrap(); // gen 2
    fs.write("/", "/b.txt", b"gen3 state", false).unwrap(); // gen 3, slot 1
    let mut img = fs.into_inner();

    // Corrupt the *live* checkpoint slot (simulates a torn write of gen 3).
    let gen3_slot = 1 + ((3 + 1) % 2); // slot_for(3) == block 1
    let mut block = vec![0u8; BLOCK_SIZE];
    img.read_block(gen3_slot, &mut block).unwrap();
    block[2000] ^= 0xff;
    img.write_block(gen3_slot, &block).unwrap();

    // Mount falls back to gen 2: a.txt exists, b.txt does not.
    let mut fs = Tinyfs::mount(img).unwrap();
    assert_eq!(fs.stats().generation, 2);
    assert_eq!(fs.read("/", "/a.txt").unwrap(), b"gen2 state");
    assert_eq!(fs.read("/", "/b.txt"), Err(FsError::NotFound));
    fs.check().unwrap();
}
