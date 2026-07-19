//! Read-only FAT12/16/32 with MBR and VFAT long names — just enough to load
//! app binaries from the ESP that QEMU synthesizes (vvfat: MBR + FAT16 at
//! partition LBA 63 by default; all three variants handled anyway).

use alloc::string::String;
use alloc::vec::Vec;

use crate::drivers::virtioblk::{SECTOR, VirtioBlk};

#[derive(Clone, Copy, PartialEq)]
enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

pub struct DirEntry {
    pub name: String,
    pub size: usize,
    pub is_dir: bool,
    first_cluster: u32,
}

pub struct FatFs {
    blk: VirtioBlk,
    part_lba: u64,
    spc: usize,            // sectors per cluster
    fat: Vec<u8>,          // whole FAT, cached
    ftype: FatType,
    root_lba: u64,         // FAT12/16 fixed root region
    root_sectors: usize,
    root_cluster: u32,     // FAT32
    data_lba: u64,
    cluster_count: u32,
}

impl FatFs {
    pub fn mount() -> Option<Self> {
        let mut blk = VirtioBlk::init()?;
        let mut sec0 = alloc::vec![0u8; SECTOR];
        if !blk.read_sectors(0, 1, &mut sec0) {
            return None;
        }
        if sec0[510] != 0x55 || sec0[511] != 0xAA {
            return None;
        }
        // MBR vs bare VBR: a VBR starts with a jump and has a plausible BPB.
        let part_lba = if matches!(sec0[0], 0xEB | 0xE9) {
            0
        } else {
            u32::from_le_bytes(sec0[0x1BE + 8..0x1BE + 12].try_into().unwrap()) as u64
        };
        let mut vbr = alloc::vec![0u8; SECTOR];
        if !blk.read_sectors(part_lba, 1, &mut vbr) {
            return None;
        }

        let u16le = |b: &[u8], o: usize| u16::from_le_bytes(b[o..o + 2].try_into().unwrap()) as u32;
        let u32le = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let bps = u16le(&vbr, 11) as usize;
        if bps != SECTOR {
            return None; // keep it simple: 512-byte sectors only
        }
        let spc = vbr[13] as usize;
        let reserved = u16le(&vbr, 14) as u64;
        let nfats = vbr[16] as u64;
        let root_entries = u16le(&vbr, 17) as usize;
        let fat_size = match u16le(&vbr, 22) {
            0 => u32le(&vbr, 36) as u64, // FAT32
            s => s as u64,
        };
        let total = match u16le(&vbr, 19) {
            0 => u32le(&vbr, 32) as u64,
            s => s as u64,
        };

        let root_sectors = (root_entries * 32).div_ceil(SECTOR);
        let fat_lba = part_lba + reserved;
        let root_lba = fat_lba + nfats * fat_size;
        let data_lba = root_lba + root_sectors as u64;
        let cluster_count = ((total - (data_lba - part_lba)) / spc as u64) as u32;
        let ftype = if cluster_count < 4085 {
            FatType::Fat12
        } else if cluster_count < 65525 {
            FatType::Fat16
        } else {
            FatType::Fat32
        };

        let mut fat = alloc::vec![0u8; fat_size as usize * SECTOR];
        if !blk.read_sectors(fat_lba, fat_size as usize, &mut fat) {
            return None;
        }

        Some(Self {
            blk,
            part_lba,
            spc,
            fat,
            ftype,
            root_lba,
            root_sectors,
            root_cluster: if ftype == FatType::Fat32 { u32le(&vbr, 44) } else { 0 },
            data_lba,
            cluster_count,
        })
    }

    fn fat_entry(&self, cluster: u32) -> u32 {
        match self.ftype {
            FatType::Fat12 => {
                let i = cluster as usize * 3 / 2;
                let v = u16::from_le_bytes([self.fat[i], self.fat[i + 1]]);
                if cluster & 1 == 0 { (v & 0xFFF) as u32 } else { (v >> 4) as u32 }
            }
            FatType::Fat16 => {
                let i = cluster as usize * 2;
                u16::from_le_bytes([self.fat[i], self.fat[i + 1]]) as u32
            }
            FatType::Fat32 => {
                let i = cluster as usize * 4;
                u32::from_le_bytes(self.fat[i..i + 4].try_into().unwrap()) & 0x0FFF_FFFF
            }
        }
    }

    fn end_of_chain(&self, entry: u32) -> bool {
        match self.ftype {
            FatType::Fat12 => entry >= 0xFF8,
            FatType::Fat16 => entry >= 0xFFF8,
            FatType::Fat32 => entry >= 0x0FFF_FFF8,
        }
    }

    fn read_cluster(&mut self, cluster: u32, out: &mut [u8]) -> bool {
        let lba = self.data_lba + (cluster as u64 - 2) * self.spc as u64;
        self.blk.read_sectors(lba, self.spc, out)
    }

    /// All bytes of a cluster chain.
    fn read_chain(&mut self, mut cluster: u32) -> Option<Vec<u8>> {
        let bpc = self.spc * SECTOR;
        let mut out = Vec::new();
        let mut hops = 0;
        while cluster >= 2 && !self.end_of_chain(cluster) {
            let at = out.len();
            out.resize(at + bpc, 0);
            if !self.read_cluster(cluster, &mut out[at..]) {
                return None;
            }
            cluster = self.fat_entry(cluster);
            hops += 1;
            if hops > self.cluster_count + 16 {
                return None; // cyclic chain
            }
        }
        Some(out)
    }

    /// Raw bytes of a directory: fixed root region or a cluster chain.
    fn dir_bytes(&mut self, dir: Option<&DirEntry>) -> Option<Vec<u8>> {
        match dir {
            None => match self.ftype {
                FatType::Fat32 => self.read_chain(self.root_cluster),
                _ => {
                    let mut out = alloc::vec![0u8; self.root_sectors * SECTOR];
                    self.blk
                        .read_sectors(self.root_lba, self.root_sectors, &mut out)
                        .then_some(out)
                }
            },
            Some(e) => self.read_chain(e.first_cluster),
        }
    }

    fn parse_dir(bytes: &[u8]) -> Vec<DirEntry> {
        let mut out = Vec::new();
        let mut lfn: Vec<(u8, String)> = Vec::new();
        for e in bytes.chunks_exact(32) {
            match e[0] {
                0x00 => break,       // end of directory
                0xE5 => {
                    lfn.clear();     // deleted
                    continue;
                }
                _ => {}
            }
            let attr = e[11];
            if attr & 0x0F == 0x0F {
                // LFN part: 13 UCS-2 chars across three fields.
                let seq = e[0] & 0x1F;
                let mut part = String::new();
                for range in [(1usize, 11usize), (14, 26), (28, 32)] {
                    for i in (range.0..range.1).step_by(2) {
                        let c = u16::from_le_bytes([e[i], e[i + 1]]);
                        if c == 0 || c == 0xFFFF {
                            continue;
                        }
                        part.push(char::from_u32(c as u32).unwrap_or('?'));
                    }
                }
                lfn.push((seq, part));
                continue;
            }
            if attr & 0x08 != 0 {
                lfn.clear(); // volume label
                continue;
            }
            let name = if !lfn.is_empty() {
                lfn.sort_by_key(|(s, _)| *s);
                let n = lfn.drain(..).map(|(_, p)| p).collect::<String>();
                n
            } else {
                let base = core::str::from_utf8(&e[0..8]).unwrap_or("").trim_end();
                let ext = core::str::from_utf8(&e[8..11]).unwrap_or("").trim_end();
                if ext.is_empty() {
                    base.to_lowercase()
                } else {
                    alloc::format!("{}.{}", base.to_lowercase(), ext.to_lowercase())
                }
            };
            let first_cluster =
                (u16::from_le_bytes([e[26], e[27]]) as u32) | (u16::from_le_bytes([e[20], e[21]]) as u32) << 16;
            out.push(DirEntry {
                name,
                size: u32::from_le_bytes(e[28..32].try_into().unwrap()) as usize,
                is_dir: attr & 0x10 != 0,
                first_cluster,
            });
        }
        out
    }

    /// Walk `path` from the root. Returns the final entry (None for the root
    /// itself) and, when it is a directory, its listing.
    fn resolve(&mut self, path: &str) -> Option<(Option<DirEntry>, Vec<DirEntry>)> {
        let mut entry: Option<DirEntry> = None;
        let mut listing = Self::parse_dir(&self.dir_bytes(None)?);
        for comp in path.split('/').filter(|c| !c.is_empty()) {
            let next = listing
                .into_iter()
                .find(|e| e.name.eq_ignore_ascii_case(comp))?;
            listing = if next.is_dir {
                Self::parse_dir(&self.dir_bytes(Some(&next))?)
            } else {
                Vec::new() // a file mid-path fails the next component's find
            };
            entry = Some(next);
        }
        Some((entry, listing))
    }

    /// Directory listing (name, size, is_dir), or None if the path is bad.
    pub fn list(&mut self, path: &str) -> Option<Vec<(String, usize, bool)>> {
        let (entry, listing) = self.resolve(path)?;
        if let Some(e) = &entry {
            if !e.is_dir {
                return Some(alloc::vec![(e.name.clone(), e.size, false)]);
            }
        }
        Some(listing.iter().map(|e| (e.name.clone(), e.size, e.is_dir)).collect())
    }

    /// Whole contents of a file.
    pub fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        let (entry, _) = self.resolve(path)?;
        let e = entry.filter(|e| !e.is_dir)?;
        let mut bytes = self.read_chain(e.first_cluster)?;
        bytes.truncate(e.size);
        Some(bytes)
    }
}
