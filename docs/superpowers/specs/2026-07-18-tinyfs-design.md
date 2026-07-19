# tinyfs — Native CoW Filesystem — Design Spec

Date: 2026-07-18
Status: implemented

## Goal

Give tinyOS its first persistent storage: **tinyfs**, a simple copy-on-write
native filesystem that is the OS default. Files survive reboots, the terminal
grows real file commands, and the on-disk format logic becomes the repo's
first host-testable code.

## Decisions

- **Shadow-paging CoW, not a cleaning log.** No block is ever overwritten in
  place; each commit writes a new checkpoint into the slot not currently
  live. A crash at any point leaves the previous checkpoint intact (CRC
  rejects a torn slot). No journal, no fsck, and — unlike a log-structured
  design — **no garbage collector, ever**: free space is rebuilt in RAM at
  mount by walking the live checkpoint (a 64 MiB disk is 16K blocks; the walk
  is trivial).
- **Shared crate.** `tinyfs/` is `no_std` + `alloc`, generic over a
  `BlockDevice` trait. The kernel backs it with virtio-blk, the host
  `tools/mkfs-tinyfs` binary with a plain file, and `cargo test -p tinyfs`
  with an in-memory device — including a crash-consistency test that cuts
  the write stream at every point and proves each prefix mounts as either
  the old or the new tree.
- **Write-through.** Every mutating shell command ends with a commit
  (data → flush → checkpoint to alternate slot → flush). Batching deferred.
- **Polled, synchronous IO.** virtio-blk requests are single-outstanding,
  spin-polled 3-descriptor chains; INTx is masked at the PCI command register
  (a polled device on a level-triggered shared line would interrupt-storm).
  The FS spinlock is therefore never held across a scheduler yield.

## On-disk format (all little-endian, 4096-byte blocks)

```
block 0      StaticHeader   magic "tinyfs\x01\0", version, block size,
                            total blocks, inode count, CRC-32; written once
block 1      Checkpoint A   magic "tfckpt\x01\0", generation, itab[128]
block 2      Checkpoint B   (block addr per inode-table block), CRC-32;
                            mount picks the valid slot with highest gen
block 3..    CoW-allocated  inode-table blocks, indirect blocks, data
```

- **Inodes:** 4096 × 128 B (32 per table block). kind (file/dir), size,
  mtime_ms, 11 direct + 1 single-indirect + 1 double-indirect pointer block
  (11 + 512 + 512² blocks) → max file ~1 GiB. Inode 1 is the root directory.
- **Directories:** file content is packed 64 B dirents (ino u32, kind, name
  ≤ 56 bytes), linear scan, compacted on remove.
- **Free space:** no on-disk allocator state. In-RAM bitmap rebuilt at mount;
  blocks freed by CoW sit in a `pending_free` list and rejoin the allocator
  only after the next checkpoint commit (the shadow-paging rule).
- **Commit invariant:** write all new data/indirect/inode-table blocks →
  flush → write checkpoint gen+1 to the non-live slot → flush → release
  pending frees. This ordering is the entire crash story.

## Integration

- QEMU gains `-device virtio-blk-pci,drive=hd,disable-legacy=on` over
  `disk.img` (created on demand by `make`, persists across runs,
  `make cleandisk` resets). `disable-legacy` matters: it makes our disk
  enumerate as modern id 0x1042 while the VVFAT ESP drive stays legacy
  0x1001 on aarch64, so the driver can never claim the boot volume — and
  mount additionally verifies the tinyfs magic before touching anything.
- `drivers::probe()` does one PCI scan with one BAR allocator (two
  allocators would hand out overlapping BARs) and claims input + blk.
- Terminal built-ins: `ls cat write append mkdir rm [-r] mv cd pwd fsinfo`,
  with a real cwd in the prompt.
- Kernel wrappers in `kernel/src/fs/` hold `spin::Mutex<Option<Tinyfs<BlkDev>>>`;
  every op is synchronous, so the lock never crosses a yield.

## Deferred

- Data-block checksums, block
  cache, batched commits, hard links, permissions/ownership, timestamps
  beyond mtime, WaitQueue-based async IO (requires moving the FS off spin
  locks). GC-free is permanent by design, not deferred.
