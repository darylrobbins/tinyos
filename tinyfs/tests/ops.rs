//! Filesystem operations on an in-memory device, including remount
//! persistence and the indirect-block path.

use tinyfs::{FsError, InodeKind, MemDevice, Tinyfs, MAX_FILE_SIZE};

fn fresh() -> Tinyfs<MemDevice> {
    Tinyfs::format(MemDevice::new(2048)).unwrap()
}

#[test]
fn format_and_mount_empty() {
    let fs = fresh();
    let dev = fs.into_inner();
    let mut fs = Tinyfs::mount(dev).unwrap();
    assert!(fs.list("/", "/").unwrap().is_empty());
    let stats = fs.check().unwrap();
    assert_eq!(stats.inodes_used, 1); // root
    assert_eq!(stats.generation, 1);
}

#[test]
fn write_read_roundtrip() {
    let mut fs = fresh();
    fs.write("/", "/hello.txt", b"hello tinyfs", false).unwrap();
    assert_eq!(fs.read("/", "/hello.txt").unwrap(), b"hello tinyfs");
    // Overwrite shrinks/replaces.
    fs.write("/", "/hello.txt", b"bye", false).unwrap();
    assert_eq!(fs.read("/", "/hello.txt").unwrap(), b"bye");
    // Append.
    fs.write("/", "/hello.txt", b" for now", true).unwrap();
    assert_eq!(fs.read("/", "/hello.txt").unwrap(), b"bye for now");
    fs.check().unwrap();
}

#[test]
fn dirs_and_relative_paths() {
    let mut fs = fresh();
    fs.mkdir("/", "/a").unwrap();
    fs.mkdir("/", "/a/b").unwrap();
    fs.write("/a/b", "c.txt", b"nested", false).unwrap();
    assert_eq!(fs.read("/", "/a/b/c.txt").unwrap(), b"nested");
    assert_eq!(fs.read("/a", "b/c.txt").unwrap(), b"nested");
    assert_eq!(fs.read("/a/b", "../b/./c.txt").unwrap(), b"nested");

    let ls = fs.list("/", "/a").unwrap();
    assert_eq!(ls.len(), 1);
    assert_eq!(ls[0].name, "b");
    assert_eq!(ls[0].kind, InodeKind::Dir);

    // ls sorts dirs first.
    fs.write("/", "/a/aaa.txt", b"x", false).unwrap();
    let ls = fs.list("/", "/a").unwrap();
    assert_eq!(ls[0].name, "b");
    assert_eq!(ls[1].name, "aaa.txt");
    fs.check().unwrap();
}

#[test]
fn large_file_uses_indirect_blocks() {
    let mut fs = fresh();
    // 100 blocks worth: well past the 11 direct pointers.
    let data: Vec<u8> = (0..409_600usize).map(|i| (i % 251) as u8).collect();
    fs.write("/", "/big.bin", &data, false).unwrap();
    assert_eq!(fs.read("/", "/big.bin").unwrap(), data);
    fs.check().unwrap();

    // Still intact after remount.
    let mut fs = Tinyfs::mount(fs.into_inner()).unwrap();
    assert_eq!(fs.read("/", "/big.bin").unwrap(), data);
    fs.check().unwrap();

    // And its blocks come back when deleted.
    let free_before = fs.stats().free_blocks;
    fs.remove("/", "/big.bin", false).unwrap();
    assert!(fs.stats().free_blocks > free_before + 100);
    fs.check().unwrap();
}

#[test]
fn double_indirect_boundary() {
    const BS: usize = 4096;
    const SINGLE_MAX: usize = 11 + 512; // blocks coverable without dindirect
    let mut fs = Tinyfs::format(MemDevice::new(4096)).unwrap();

    // Exactly at the single-indirect limit: no double-indirect tree.
    let data: Vec<u8> = (0..SINGLE_MAX * BS).map(|i| (i % 253) as u8).collect();
    fs.write("/", "/edge.bin", &data, false).unwrap();
    let used_single = fs.stats().used_blocks;

    // One byte over: crosses into double-indirect (one more data block,
    // plus the L1 and one L2 pointer block).
    fs.write("/", "/edge.bin", &[&data[..], &[0xAB][..]].concat(), false)
        .unwrap();
    assert_eq!(fs.stats().used_blocks, used_single + 3);
    let back = fs.read("/", "/edge.bin").unwrap();
    assert_eq!(back.len(), SINGLE_MAX * BS + 1);
    assert_eq!(*back.last().unwrap(), 0xAB);
    assert_eq!(&back[..data.len()], &data[..]);
    fs.check().unwrap();

    // Well into the L2 tree (600 blocks), remount, verify, delete, reclaim.
    let data: Vec<u8> = (0..600 * BS).map(|i| (i % 249) as u8).collect();
    fs.write("/", "/edge.bin", &data, false).unwrap();
    let mut fs = Tinyfs::mount(fs.into_inner()).unwrap();
    assert_eq!(fs.read("/", "/edge.bin").unwrap(), data);
    fs.check().unwrap();
    let free_before = fs.stats().free_blocks;
    fs.remove("/", "/edge.bin", false).unwrap();
    assert!(fs.stats().free_blocks >= free_before + 600);
    fs.check().unwrap();
}

#[test]
fn file_too_big() {
    // ~1 GiB limit: 11 direct + 512 single + 512*512 double-indirect blocks.
    // (Exercising FileTooBig for real would need a >1 GiB buffer; the
    // boundary test above covers the tree shape, so pin the constant and
    // check that an over-device write fails as NoSpace before corruption.)
    assert_eq!(MAX_FILE_SIZE, (11 + 512 + 512 * 512) as u64 * 4096);
    let mut fs = Tinyfs::format(MemDevice::new(256)).unwrap();
    let data = vec![0u8; 300 * 4096]; // > device, < MAX_FILE_SIZE
    assert_eq!(fs.write("/", "/huge", &data, false), Err(FsError::NoSpace));
}

#[test]
fn remove_and_rename() {
    let mut fs = fresh();
    fs.mkdir("/", "/d").unwrap();
    fs.write("/", "/d/f1", b"one", false).unwrap();
    fs.write("/", "/d/f2", b"two", false).unwrap();

    assert_eq!(fs.remove("/", "/d", false), Err(FsError::NotEmpty));
    fs.remove("/", "/d/f1", false).unwrap();
    assert_eq!(fs.read("/", "/d/f1"), Err(FsError::NotFound));

    fs.rename("/", "/d/f2", "/renamed").unwrap();
    assert_eq!(fs.read("/", "/renamed").unwrap(), b"two");
    assert!(fs.list("/", "/d").unwrap().is_empty());

    fs.remove("/", "/d", false).unwrap();
    assert_eq!(fs.list("/", "/d"), Err(FsError::NotFound));

    // Recursive remove.
    fs.mkdir("/", "/tree").unwrap();
    fs.mkdir("/", "/tree/sub").unwrap();
    fs.write("/", "/tree/sub/leaf", b"leaf", false).unwrap();
    fs.remove("/", "/tree", true).unwrap();
    assert_eq!(fs.lookup("/", "/tree"), Err(FsError::NotFound));
    fs.check().unwrap();
}

#[test]
fn rename_guards() {
    let mut fs = fresh();
    fs.mkdir("/", "/a").unwrap();
    fs.mkdir("/", "/a/b").unwrap();
    // Can't move a directory into its own subtree.
    assert_eq!(fs.rename("/", "/a", "/a/b/a"), Err(FsError::InvalidPath));
    // Can't clobber an existing name.
    fs.write("/", "/x", b"x", false).unwrap();
    fs.write("/", "/y", b"y", false).unwrap();
    assert_eq!(fs.rename("/", "/x", "/y"), Err(FsError::Exists));
    // Same-directory rename works.
    fs.rename("/", "/x", "/x2").unwrap();
    assert_eq!(fs.read("/", "/x2").unwrap(), b"x");
    fs.check().unwrap();
}

#[test]
fn error_cases() {
    let mut fs = fresh();
    assert_eq!(fs.read("/", "/nope"), Err(FsError::NotFound));
    fs.mkdir("/", "/d").unwrap();
    assert_eq!(fs.read("/", "/d"), Err(FsError::IsADir));
    assert_eq!(fs.mkdir("/", "/d"), Err(FsError::Exists));
    fs.write("/", "/f", b"f", false).unwrap();
    assert_eq!(fs.list("/", "/f"), Err(FsError::NotADir));
    assert_eq!(fs.write("/", "/f/x", b"", false), Err(FsError::NotADir));
    assert_eq!(fs.write("/", "/d", b"", false), Err(FsError::IsADir));
    let long = "n".repeat(57);
    assert_eq!(fs.mkdir("/", &long), Err(FsError::NameTooLong));
}

#[test]
fn persistence_across_many_remounts() {
    let mut fs = fresh();
    for gen in 0..20 {
        let name = format!("/file{gen}");
        fs.write("/", &name, format!("content {gen}").as_bytes(), false)
            .unwrap();
        fs = Tinyfs::mount(fs.into_inner()).unwrap();
        for old in 0..=gen {
            let name = format!("/file{old}");
            assert_eq!(
                fs.read("/", &name).unwrap(),
                format!("content {old}").as_bytes()
            );
        }
    }
    fs.check().unwrap();
}

#[test]
fn space_is_reclaimed_not_leaked() {
    let mut fs = fresh();
    let base = fs.stats().used_blocks;
    // Churn the same file many times; shadow paging must recycle blocks.
    for i in 0..200 {
        fs.write("/", "/churn", vec![i as u8; 8192].as_slice(), false)
            .unwrap();
    }
    let after = fs.stats().used_blocks;
    // 2 data blocks + an inode-table block + root dir block above baseline.
    assert!(after <= base + 8, "leaked blocks: {base} -> {after}");
    fs.remove("/", "/churn", false).unwrap();
    fs.check().unwrap();
}

#[test]
fn mount_rejects_garbage() {
    assert_eq!(
        Tinyfs::mount(MemDevice::new(2048)).err(),
        Some(FsError::Corrupt)
    );
    let mut small = MemDevice::new(8);
    assert_eq!(Tinyfs::format(small.clone()).err(), Some(FsError::NoSpace));
    use tinyfs::BlockDevice;
    let _ = small.read_block(0, &mut [0u8; 4096]);
}
