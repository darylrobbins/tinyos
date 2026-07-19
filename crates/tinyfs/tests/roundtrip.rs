//! On-disk format round-trip and CRC vectors.

use tinyfs::crc::crc32;
use tinyfs::layout::*;

#[test]
fn crc32_vector() {
    // The classic IEEE check value.
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn header_roundtrip() {
    let hdr = StaticHeader {
        total_blocks: 16384,
    };
    let mut block = vec![0u8; BLOCK_SIZE];
    hdr.encode(&mut block);
    assert_eq!(StaticHeader::decode(&block).unwrap(), hdr);

    // Any bit flip must be caught by the CRC.
    let mut bad = block.clone();
    bad[17] ^= 1;
    assert_eq!(StaticHeader::decode(&bad), Err(FsError::Corrupt));
    let mut bad = block.clone();
    bad[4000] ^= 0x80;
    assert_eq!(StaticHeader::decode(&bad), Err(FsError::Corrupt));
}

#[test]
fn checkpoint_roundtrip() {
    let mut itab = [0u64; ITAB_BLOCKS];
    itab[0] = 42;
    itab[127] = 9999;
    let ck = Checkpoint {
        generation: 7,
        used_blocks: 1234,
        itab,
    };
    let mut block = vec![0u8; BLOCK_SIZE];
    ck.encode(&mut block);
    assert_eq!(Checkpoint::decode(&block).unwrap(), ck);
    assert_eq!(Checkpoint::slot_for(1), 1);
    assert_eq!(Checkpoint::slot_for(2), 2);
    assert_eq!(Checkpoint::slot_for(3), 1);

    let mut bad = block.clone();
    bad[100] ^= 1;
    assert_eq!(Checkpoint::decode(&bad), Err(FsError::Corrupt));
}

#[test]
fn inode_roundtrip() {
    let mut inode = Inode::empty();
    inode.kind = InodeKind::File;
    inode.size = 123456;
    inode.mtime_ms = 987654321;
    inode.direct[0] = 10;
    inode.direct[10] = 21;
    inode.indirect = 300;
    inode.dindirect = 301;
    let mut buf = vec![0u8; INODE_SIZE];
    inode.encode(&mut buf);
    assert_eq!(Inode::decode(&buf).unwrap(), inode);
}

#[test]
fn dirent_roundtrip() {
    let d = Dirent::new(17, InodeKind::Dir, "hello.txt").unwrap();
    let mut buf = vec![0u8; DIRENT_SIZE];
    d.encode(&mut buf);
    let back = Dirent::decode(&buf).unwrap();
    assert_eq!(back, d);
    assert_eq!(back.name_str().unwrap(), "hello.txt");

    assert_eq!(
        Dirent::new(1, InodeKind::File, &"x".repeat(57)),
        Err(FsError::NameTooLong)
    );
    assert!(Dirent::new(1, InodeKind::File, &"x".repeat(56)).is_ok());
    assert_eq!(Dirent::new(1, InodeKind::File, ""), Err(FsError::NameTooLong));
}
