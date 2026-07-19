//! Core filesystem: shadow-paging CoW over a `BlockDevice`.
//!
//! The whole inode table (4096 x 128 B = 512 KiB max, usually far less
//! touched) is held in RAM; mutating operations edit it in place, mark the
//! affected table blocks dirty, and finish with `commit()`:
//!
//!   1. every dirty inode-table block is written CoW to a fresh block
//!   2. flush            (all new data + metadata durable)
//!   3. checkpoint{gen+1} written to the slot NOT currently live
//!   4. flush            (the flip is durable)
//!   5. blocks freed by CoW rejoin the allocator
//!
//! A crash before step 4 completes leaves the old checkpoint intact; the
//! CRC rejects a torn slot. If a mutating operation returns an error the
//! in-memory state may be ahead of disk — remount to resynchronize (the
//! on-disk image is always the last committed tree).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::device::{BlockDevice, BLOCK_SIZE};
use crate::layout::*;
use crate::path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntryInfo {
    pub name: String,
    pub ino: u32,
    pub kind: InodeKind,
    pub size: u64,
    pub mtime_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsStats {
    pub total_blocks: u64,
    pub used_blocks: u64,
    pub free_blocks: u64,
    pub inodes_used: u32,
    pub inodes_total: u32,
    pub generation: u64,
}

fn zero_ms() -> u64 {
    0
}

pub struct Tinyfs<D: BlockDevice> {
    dev: D,
    total_blocks: u64,
    generation: u64,
    itab: [u64; ITAB_BLOCKS],
    itab_dirty: [bool; ITAB_BLOCKS],
    inodes: Vec<Inode>,
    /// bit set = block in use (including blocks awaiting `pending_free`).
    bitmap: Vec<u64>,
    /// Blocks superseded by CoW this transaction; reusable only after commit.
    pending_free: Vec<u64>,
    cursor: u64,
    now_ms: fn() -> u64,
}

impl<D: BlockDevice> Tinyfs<D> {
    // -- construction ---------------------------------------------------------

    pub fn format(mut dev: D) -> Result<Self, FsError> {
        let total_blocks = dev.block_count();
        if total_blocks < MIN_BLOCKS {
            return Err(FsError::NoSpace);
        }
        let mut block = vec![0u8; BLOCK_SIZE];
        StaticHeader { total_blocks }.encode(&mut block);
        dev.write_block(0, &block)?;
        // Invalidate both checkpoint slots so a stale image can't resurrect.
        block.fill(0);
        dev.write_block(1, &block)?;
        dev.write_block(2, &block)?;
        dev.flush()?;

        let mut fs = Self {
            dev,
            total_blocks,
            generation: 0,
            itab: [0; ITAB_BLOCKS],
            itab_dirty: [false; ITAB_BLOCKS],
            inodes: vec![Inode::empty(); INODE_COUNT],
            bitmap: vec![0; (total_blocks as usize + 63) / 64],
            pending_free: Vec::new(),
            cursor: FIRST_DATA_BLOCK,
            now_ms: zero_ms,
        };
        for b in 0..FIRST_DATA_BLOCK {
            fs.bset(b);
        }
        fs.inodes[ROOT_INO as usize] = Inode {
            kind: InodeKind::Dir,
            ..Inode::empty()
        };
        fs.itab_dirty[ROOT_INO as usize / INODES_PER_BLOCK] = true;
        fs.commit()?;
        Ok(fs)
    }

    pub fn mount(mut dev: D) -> Result<Self, FsError> {
        let mut block = vec![0u8; BLOCK_SIZE];
        dev.read_block(0, &mut block)?;
        let hdr = StaticHeader::decode(&block)?;
        if hdr.total_blocks < MIN_BLOCKS || hdr.total_blocks > dev.block_count() {
            return Err(FsError::Corrupt);
        }

        let mut best: Option<Checkpoint> = None;
        for slot in [1u64, 2] {
            dev.read_block(slot, &mut block)?;
            if let Ok(ck) = Checkpoint::decode(&block) {
                if best.as_ref().map_or(true, |b| ck.generation > b.generation) {
                    best = Some(ck);
                }
            }
        }
        let ck = best.ok_or(FsError::Corrupt)?;

        let mut fs = Self {
            dev,
            total_blocks: hdr.total_blocks,
            generation: ck.generation,
            itab: ck.itab,
            itab_dirty: [false; ITAB_BLOCKS],
            inodes: vec![Inode::empty(); INODE_COUNT],
            bitmap: vec![0; (hdr.total_blocks as usize + 63) / 64],
            pending_free: Vec::new(),
            cursor: FIRST_DATA_BLOCK,
            now_ms: zero_ms,
        };
        for b in 0..FIRST_DATA_BLOCK {
            fs.bset(b);
        }

        // Load the inode table and rebuild the free-space bitmap by walking
        // everything reachable from the checkpoint.
        for i in 0..ITAB_BLOCKS {
            let b = fs.itab[i];
            if b == 0 {
                continue;
            }
            fs.check_range(b)?;
            fs.bset(b);
            fs.dev.read_block(b, &mut block)?;
            for j in 0..INODES_PER_BLOCK {
                fs.inodes[i * INODES_PER_BLOCK + j] =
                    Inode::decode(&block[j * INODE_SIZE..(j + 1) * INODE_SIZE])?;
            }
        }
        for i in 0..INODE_COUNT {
            if fs.inodes[i].kind == InodeKind::Free {
                continue;
            }
            let blocks = fs.inode_blocks(i)?;
            for b in blocks {
                fs.check_range(b)?;
                fs.bset(b);
            }
            let ind = fs.inodes[i].indirect;
            if ind != 0 {
                fs.check_range(ind)?;
                fs.bset(ind);
            }
        }
        if fs.inodes[ROOT_INO as usize].kind != InodeKind::Dir {
            return Err(FsError::Corrupt);
        }
        Ok(fs)
    }

    /// Millisecond clock used to stamp mtimes.
    pub fn set_time_fn(&mut self, f: fn() -> u64) {
        self.now_ms = f;
    }

    pub fn into_inner(self) -> D {
        self.dev
    }

    // -- public operations ------------------------------------------------------

    pub fn lookup(&mut self, cwd: &str, p: &str) -> Result<(u32, Inode), FsError> {
        let comps = path::resolve(cwd, p)?;
        let ino = self.walk(&comps)?;
        Ok((ino, self.inodes[ino as usize]))
    }

    pub fn read(&mut self, cwd: &str, p: &str) -> Result<Vec<u8>, FsError> {
        let (ino, inode) = self.lookup(cwd, p)?;
        if inode.kind != InodeKind::File {
            return Err(FsError::IsADir);
        }
        self.read_file(ino as usize)
    }

    pub fn write(&mut self, cwd: &str, p: &str, data: &[u8], append: bool) -> Result<(), FsError> {
        let comps = path::resolve(cwd, p)?;
        let (name, parent_comps) = match comps.split_last() {
            Some((n, rest)) => (n.clone(), rest),
            None => return Err(FsError::IsADir), // "/"
        };
        let parent = self.walk(parent_comps)?;
        let existing = self.dir_find(parent as usize, &name)?;
        let ino = match existing {
            Some(ref e) => {
                if e.kind != InodeKind::File {
                    return Err(FsError::IsADir);
                }
                e.ino
            }
            None => {
                let ino = self.alloc_ino(InodeKind::File)?;
                self.dir_add(parent as usize, Dirent::new(ino, InodeKind::File, &name)?)?;
                ino
            }
        };
        let content = if append && existing.is_some() {
            let mut old = self.read_file(ino as usize)?;
            old.extend_from_slice(data);
            old
        } else {
            data.to_vec()
        };
        self.set_file_data(ino as usize, &content)?;
        self.commit()
    }

    pub fn list(&mut self, cwd: &str, p: &str) -> Result<Vec<DirEntryInfo>, FsError> {
        let (ino, inode) = self.lookup(cwd, p)?;
        if inode.kind != InodeKind::Dir {
            return Err(FsError::NotADir);
        }
        let mut out = Vec::new();
        for d in self.dir_entries(ino as usize)? {
            let inode = self.inodes[d.ino as usize];
            out.push(DirEntryInfo {
                name: String::from(d.name_str()?),
                ino: d.ino,
                kind: inode.kind,
                size: inode.size,
                mtime_ms: inode.mtime_ms,
            });
        }
        out.sort_by(|a, b| {
            (a.kind != InodeKind::Dir, &a.name).cmp(&(b.kind != InodeKind::Dir, &b.name))
        });
        Ok(out)
    }

    pub fn mkdir(&mut self, cwd: &str, p: &str) -> Result<(), FsError> {
        let comps = path::resolve(cwd, p)?;
        let (name, parent_comps) = comps.split_last().ok_or(FsError::Exists)?;
        let parent = self.walk(parent_comps)?;
        if self.dir_find(parent as usize, name)?.is_some() {
            return Err(FsError::Exists);
        }
        let ino = self.alloc_ino(InodeKind::Dir)?;
        self.dir_add(parent as usize, Dirent::new(ino, InodeKind::Dir, name)?)?;
        self.commit()
    }

    pub fn remove(&mut self, cwd: &str, p: &str, recursive: bool) -> Result<(), FsError> {
        let comps = path::resolve(cwd, p)?;
        let (name, parent_comps) = comps.split_last().ok_or(FsError::InvalidPath)?;
        let parent = self.walk(parent_comps)?;
        let entry = self
            .dir_find(parent as usize, name)?
            .ok_or(FsError::NotFound)?;
        if self.inodes[entry.ino as usize].kind == InodeKind::Dir {
            let children = self.dir_entries(entry.ino as usize)?;
            if !children.is_empty() && !recursive {
                return Err(FsError::NotEmpty);
            }
        }
        self.free_tree(entry.ino as usize)?;
        self.dir_remove(parent as usize, name)?;
        self.commit()
    }

    pub fn rename(&mut self, cwd: &str, from: &str, to: &str) -> Result<(), FsError> {
        let src = path::resolve(cwd, from)?;
        let dst = path::resolve(cwd, to)?;
        let (src_name, src_parent_comps) = src.split_last().ok_or(FsError::InvalidPath)?;
        let (dst_name, dst_parent_comps) = dst.split_last().ok_or(FsError::InvalidPath)?;
        // A directory must not move into its own subtree.
        if dst.len() > src.len() && dst[..src.len()] == src[..] {
            return Err(FsError::InvalidPath);
        }
        let src_parent = self.walk(src_parent_comps)?;
        let dst_parent = self.walk(dst_parent_comps)?;
        let entry = self
            .dir_find(src_parent as usize, src_name)?
            .ok_or(FsError::NotFound)?;
        if self.dir_find(dst_parent as usize, dst_name)?.is_some() {
            return Err(FsError::Exists);
        }
        let moved = Dirent::new(entry.ino, entry.kind, dst_name)?;
        if src_parent == dst_parent {
            let mut entries = self.dir_entries(src_parent as usize)?;
            entries.retain(|d| d.name_str().map(|n| n != src_name).unwrap_or(true));
            entries.push(moved);
            self.dir_write(src_parent as usize, &entries)?;
        } else {
            self.dir_remove(src_parent as usize, src_name)?;
            self.dir_add(dst_parent as usize, moved)?;
        }
        self.commit()
    }

    pub fn stats(&self) -> FsStats {
        let used = self
            .bitmap
            .iter()
            .map(|w| w.count_ones() as u64)
            .sum::<u64>();
        FsStats {
            total_blocks: self.total_blocks,
            used_blocks: used,
            free_blocks: self.total_blocks - used,
            inodes_used: self
                .inodes
                .iter()
                .filter(|i| i.kind != InodeKind::Free)
                .count() as u32,
            inodes_total: INODE_COUNT as u32,
            generation: self.generation,
        }
    }

    /// Structural integrity walk from the root. Cheap paranoia for the host
    /// tool's `check` command and the tests.
    pub fn check(&mut self) -> Result<FsStats, FsError> {
        let mut stack = vec![ROOT_INO];
        let mut seen_inos = vec![false; INODE_COUNT];
        while let Some(ino) = stack.pop() {
            let idx = ino as usize;
            if idx == 0 || idx >= INODE_COUNT || seen_inos[idx] {
                return Err(FsError::Corrupt);
            }
            seen_inos[idx] = true;
            let inode = self.inodes[idx];
            match inode.kind {
                InodeKind::Free => return Err(FsError::Corrupt),
                InodeKind::File => {
                    for b in self.inode_blocks(idx)? {
                        self.check_range(b)?;
                    }
                }
                InodeKind::Dir => {
                    for d in self.dir_entries(idx)? {
                        if self.inodes[d.ino as usize].kind != d.kind {
                            return Err(FsError::Corrupt);
                        }
                        stack.push(d.ino);
                    }
                }
            }
        }
        // Every allocated inode must be reachable.
        for i in 1..INODE_COUNT {
            if self.inodes[i].kind != InodeKind::Free && !seen_inos[i] {
                return Err(FsError::Corrupt);
            }
        }
        Ok(self.stats())
    }

    // -- inode & block internals -------------------------------------------------

    fn check_range(&self, b: u64) -> Result<(), FsError> {
        if b < FIRST_DATA_BLOCK || b >= self.total_blocks {
            return Err(FsError::Corrupt);
        }
        Ok(())
    }

    fn bset(&mut self, b: u64) {
        self.bitmap[b as usize / 64] |= 1 << (b % 64);
    }

    fn bclr(&mut self, b: u64) {
        self.bitmap[b as usize / 64] &= !(1 << (b % 64));
    }

    fn bget(&self, b: u64) -> bool {
        self.bitmap[b as usize / 64] & (1 << (b % 64)) != 0
    }

    fn alloc_block(&mut self) -> Result<u64, FsError> {
        let span = self.total_blocks - FIRST_DATA_BLOCK;
        for i in 0..span {
            let b = FIRST_DATA_BLOCK + (self.cursor - FIRST_DATA_BLOCK + i) % span;
            if !self.bget(b) {
                self.bset(b);
                self.cursor = b + 1;
                if self.cursor >= self.total_blocks {
                    self.cursor = FIRST_DATA_BLOCK;
                }
                return Ok(b);
            }
        }
        Err(FsError::NoSpace)
    }

    fn alloc_ino(&mut self, kind: InodeKind) -> Result<u32, FsError> {
        for i in 1..INODE_COUNT {
            if self.inodes[i].kind == InodeKind::Free {
                self.inodes[i] = Inode {
                    kind,
                    mtime_ms: (self.now_ms)(),
                    ..Inode::empty()
                };
                self.itab_dirty[i / INODES_PER_BLOCK] = true;
                return Ok(i as u32);
            }
        }
        Err(FsError::NoInodes)
    }

    /// Data blocks of an inode, in order (excluding the indirect table block).
    fn inode_blocks(&mut self, idx: usize) -> Result<Vec<u64>, FsError> {
        let inode = self.inodes[idx];
        let n = (inode.size as usize).div_ceil(BLOCK_SIZE);
        if n > MAX_FILE_BLOCKS {
            return Err(FsError::Corrupt);
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n.min(DIRECT_PTRS) {
            if inode.direct[i] == 0 {
                return Err(FsError::Corrupt);
            }
            out.push(inode.direct[i]);
        }
        if n > DIRECT_PTRS {
            if inode.indirect == 0 {
                return Err(FsError::Corrupt);
            }
            self.check_range(inode.indirect)?;
            let mut block = vec![0u8; BLOCK_SIZE];
            self.dev.read_block(inode.indirect, &mut block)?;
            for i in 0..n - DIRECT_PTRS {
                let b = u64::from_le_bytes(block[i * 8..i * 8 + 8].try_into().unwrap());
                if b == 0 {
                    return Err(FsError::Corrupt);
                }
                out.push(b);
            }
        }
        Ok(out)
    }

    fn read_file(&mut self, idx: usize) -> Result<Vec<u8>, FsError> {
        let size = self.inodes[idx].size as usize;
        let blocks = self.inode_blocks(idx)?;
        let mut out = Vec::with_capacity(size);
        let mut block = vec![0u8; BLOCK_SIZE];
        for b in blocks {
            self.check_range(b)?;
            self.dev.read_block(b, &mut block)?;
            let take = (size - out.len()).min(BLOCK_SIZE);
            out.extend_from_slice(&block[..take]);
        }
        if out.len() != size {
            return Err(FsError::Corrupt);
        }
        Ok(out)
    }

    /// Replace an inode's content wholesale (CoW: old blocks are freed only
    /// after the next commit).
    fn set_file_data(&mut self, idx: usize, data: &[u8]) -> Result<(), FsError> {
        if data.len() as u64 > MAX_FILE_SIZE {
            return Err(FsError::FileTooBig);
        }
        let n = data.len().div_ceil(BLOCK_SIZE);

        let old_blocks = self.inode_blocks(idx)?;
        let old_indirect = self.inodes[idx].indirect;

        let mut new_blocks = Vec::with_capacity(n);
        for _ in 0..n {
            new_blocks.push(self.alloc_block()?);
        }
        let new_indirect = if n > DIRECT_PTRS {
            self.alloc_block()?
        } else {
            0
        };

        let mut block = vec![0u8; BLOCK_SIZE];
        for (i, &b) in new_blocks.iter().enumerate() {
            block.fill(0);
            let start = i * BLOCK_SIZE;
            let end = (start + BLOCK_SIZE).min(data.len());
            block[..end - start].copy_from_slice(&data[start..end]);
            self.dev.write_block(b, &block)?;
        }
        if new_indirect != 0 {
            block.fill(0);
            for (i, &b) in new_blocks[DIRECT_PTRS..].iter().enumerate() {
                block[i * 8..i * 8 + 8].copy_from_slice(&b.to_le_bytes());
            }
            self.dev.write_block(new_indirect, &block)?;
        }

        let inode = &mut self.inodes[idx];
        inode.direct = [0; DIRECT_PTRS];
        for (i, &b) in new_blocks.iter().take(DIRECT_PTRS).enumerate() {
            inode.direct[i] = b;
        }
        inode.indirect = new_indirect;
        inode.size = data.len() as u64;
        inode.mtime_ms = (self.now_ms)();
        self.itab_dirty[idx / INODES_PER_BLOCK] = true;

        self.pending_free.extend(old_blocks);
        if old_indirect != 0 {
            self.pending_free.push(old_indirect);
        }
        Ok(())
    }

    /// Free an inode and (for directories) everything below it.
    fn free_tree(&mut self, idx: usize) -> Result<(), FsError> {
        if self.inodes[idx].kind == InodeKind::Dir {
            for d in self.dir_entries(idx)? {
                self.free_tree(d.ino as usize)?;
            }
        }
        let blocks = self.inode_blocks(idx)?;
        self.pending_free.extend(blocks);
        if self.inodes[idx].indirect != 0 {
            let ind = self.inodes[idx].indirect;
            self.pending_free.push(ind);
        }
        self.inodes[idx] = Inode::empty();
        self.itab_dirty[idx / INODES_PER_BLOCK] = true;
        Ok(())
    }

    // -- directory internals -------------------------------------------------------

    fn dir_entries(&mut self, idx: usize) -> Result<Vec<Dirent>, FsError> {
        if self.inodes[idx].kind != InodeKind::Dir {
            return Err(FsError::NotADir);
        }
        let data = self.read_file(idx)?;
        if data.len() % DIRENT_SIZE != 0 {
            return Err(FsError::Corrupt);
        }
        let mut out = Vec::with_capacity(data.len() / DIRENT_SIZE);
        for chunk in data.chunks_exact(DIRENT_SIZE) {
            let d = Dirent::decode(chunk)?;
            if d.ino != 0 {
                out.push(d);
            }
        }
        Ok(out)
    }

    fn dir_write(&mut self, idx: usize, entries: &[Dirent]) -> Result<(), FsError> {
        let mut data = vec![0u8; entries.len() * DIRENT_SIZE];
        for (i, d) in entries.iter().enumerate() {
            d.encode(&mut data[i * DIRENT_SIZE..(i + 1) * DIRENT_SIZE]);
        }
        self.set_file_data(idx, &data)
    }

    fn dir_find(&mut self, idx: usize, name: &str) -> Result<Option<Dirent>, FsError> {
        for d in self.dir_entries(idx)? {
            if d.name_str()? == name {
                return Ok(Some(d));
            }
        }
        Ok(None)
    }

    fn dir_add(&mut self, idx: usize, entry: Dirent) -> Result<(), FsError> {
        let mut entries = self.dir_entries(idx)?;
        for d in &entries {
            if d.name_str()? == entry.name_str()? {
                return Err(FsError::Exists);
            }
        }
        entries.push(entry);
        self.dir_write(idx, &entries)
    }

    fn dir_remove(&mut self, idx: usize, name: &str) -> Result<(), FsError> {
        let mut entries = self.dir_entries(idx)?;
        let before = entries.len();
        entries.retain(|d| d.name_str().map(|n| n != name).unwrap_or(true));
        if entries.len() == before {
            return Err(FsError::NotFound);
        }
        self.dir_write(idx, &entries)
    }

    fn walk(&mut self, comps: &[String]) -> Result<u32, FsError> {
        let mut ino = ROOT_INO;
        for c in comps {
            if self.inodes[ino as usize].kind != InodeKind::Dir {
                return Err(FsError::NotADir);
            }
            ino = self
                .dir_find(ino as usize, c)?
                .ok_or(FsError::NotFound)?
                .ino;
        }
        Ok(ino)
    }

    // -- commit -------------------------------------------------------------------

    /// Make everything mutated since the last commit durable and atomic.
    /// Ordering here is the crash-consistency invariant — see module docs.
    fn commit(&mut self) -> Result<(), FsError> {
        let mut block = vec![0u8; BLOCK_SIZE];
        for i in 0..ITAB_BLOCKS {
            if !self.itab_dirty[i] {
                continue;
            }
            let b = self.alloc_block()?;
            block.fill(0);
            for j in 0..INODES_PER_BLOCK {
                self.inodes[i * INODES_PER_BLOCK + j]
                    .encode(&mut block[j * INODE_SIZE..(j + 1) * INODE_SIZE]);
            }
            self.dev.write_block(b, &block)?;
            if self.itab[i] != 0 {
                self.pending_free.push(self.itab[i]);
            }
            self.itab[i] = b;
            self.itab_dirty[i] = false;
        }

        self.dev.flush()?;

        let used = self
            .bitmap
            .iter()
            .map(|w| w.count_ones() as u64)
            .sum::<u64>()
            - self.pending_free.len() as u64;
        let gen = self.generation + 1;
        let ck = Checkpoint {
            generation: gen,
            used_blocks: used,
            itab: self.itab,
        };
        ck.encode(&mut block);
        self.dev.write_block(Checkpoint::slot_for(gen), &block)?;
        self.dev.flush()?;
        self.generation = gen;

        for b in core::mem::take(&mut self.pending_free) {
            self.bclr(b);
        }
        Ok(())
    }
}
