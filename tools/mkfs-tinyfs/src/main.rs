//! Host-side tinyfs tool: create/populate disk images, inspect and check them.
//!
//!   mkfs-tinyfs create <img> [--size 64M] [--populate <dir>]
//!   mkfs-tinyfs ls    <img> [path]
//!   mkfs-tinyfs cat   <img> <path>
//!   mkfs-tinyfs check <img>

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::exit;
use std::time::{SystemTime, UNIX_EPOCH};

use tinyfs::{BlockDevice, FsError, InodeKind, Tinyfs, BLOCK_SIZE};

struct FileDevice {
    file: File,
    blocks: u64,
}

impl FileDevice {
    fn open(path: &str, writable: bool) -> std::io::Result<Self> {
        let file = OpenOptions::new().read(true).write(writable).open(path)?;
        let blocks = file.metadata()?.len() / BLOCK_SIZE as u64;
        Ok(Self { file, blocks })
    }
}

impl BlockDevice for FileDevice {
    fn block_count(&self) -> u64 {
        self.blocks
    }
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        self.file
            .seek(SeekFrom::Start(lba * BLOCK_SIZE as u64))
            .and_then(|_| self.file.read_exact(buf))
            .map_err(|_| FsError::Io)
    }
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        self.file
            .seek(SeekFrom::Start(lba * BLOCK_SIZE as u64))
            .and_then(|_| self.file.write_all(buf))
            .map_err(|_| FsError::Io)
    }
    fn flush(&mut self) -> Result<(), FsError> {
        self.file.sync_data().map_err(|_| FsError::Io)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn parse_size(s: &str) -> Option<u64> {
    let (num, mult) = match s.chars().last()? {
        'K' | 'k' => (&s[..s.len() - 1], 1u64 << 10),
        'M' | 'm' => (&s[..s.len() - 1], 1 << 20),
        'G' | 'g' => (&s[..s.len() - 1], 1 << 30),
        _ => (s, 1),
    };
    num.parse::<u64>().ok().map(|n| n * mult)
}

fn usage() -> ! {
    eprintln!(
        "usage: mkfs-tinyfs create <img> [--size 64M] [--populate <dir>]\n\
         \x20      mkfs-tinyfs ls    <img> [path]\n\
         \x20      mkfs-tinyfs cat   <img> <path>\n\
         \x20      mkfs-tinyfs check <img>"
    );
    exit(2)
}

fn fail(msg: &str) -> ! {
    eprintln!("mkfs-tinyfs: {msg}");
    exit(1)
}

fn populate(fs: &mut Tinyfs<FileDevice>, host_dir: &std::path::Path, fs_dir: &str) {
    let entries = std::fs::read_dir(host_dir)
        .unwrap_or_else(|e| fail(&format!("read_dir {}: {e}", host_dir.display())));
    for entry in entries {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap_or_default();
        let target = if fs_dir == "/" {
            format!("/{name}")
        } else {
            format!("{fs_dir}/{name}")
        };
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            fs.mkdir("/", &target)
                .unwrap_or_else(|e| fail(&format!("mkdir {target}: {e}")));
            populate(fs, &entry.path(), &target);
        } else if ty.is_file() {
            let data = std::fs::read(entry.path())
                .unwrap_or_else(|e| fail(&format!("read {}: {e}", entry.path().display())));
            fs.write("/", &target, &data, false)
                .unwrap_or_else(|e| fail(&format!("write {target}: {e}")));
        }
    }
}

fn open_fs(img: &str, writable: bool) -> Tinyfs<FileDevice> {
    let dev = FileDevice::open(img, writable).unwrap_or_else(|e| fail(&format!("{img}: {e}")));
    let mut fs = Tinyfs::mount(dev).unwrap_or_else(|e| fail(&format!("{img}: mount: {e}")));
    fs.set_time_fn(now_ms);
    fs
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("create") => {
            let img = args.get(1).unwrap_or_else(|| usage());
            let mut size = 64u64 << 20;
            let mut populate_dir = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--size" => {
                        size = args
                            .get(i + 1)
                            .and_then(|s| parse_size(s))
                            .unwrap_or_else(|| usage());
                        i += 2;
                    }
                    "--populate" => {
                        populate_dir = Some(args.get(i + 1).cloned().unwrap_or_else(|| usage()));
                        i += 2;
                    }
                    _ => usage(),
                }
            }
            let file = File::create(img).unwrap_or_else(|e| fail(&format!("{img}: {e}")));
            file.set_len(size)
                .unwrap_or_else(|e| fail(&format!("{img}: set_len: {e}")));
            drop(file);
            let dev = FileDevice::open(img, true).unwrap();
            let mut fs =
                Tinyfs::format(dev).unwrap_or_else(|e| fail(&format!("{img}: format: {e}")));
            fs.set_time_fn(now_ms);
            if let Some(dir) = populate_dir {
                populate(&mut fs, std::path::Path::new(&dir), "/");
            }
            let st = fs.stats();
            println!(
                "{img}: tinyfs, {} blocks ({} MiB), {} used, gen {}",
                st.total_blocks,
                st.total_blocks * BLOCK_SIZE as u64 >> 20,
                st.used_blocks,
                st.generation
            );
        }
        Some("ls") => {
            let img = args.get(1).unwrap_or_else(|| usage());
            let path = args.get(2).map(String::as_str).unwrap_or("/");
            let mut fs = open_fs(img, false);
            for e in fs
                .list("/", path)
                .unwrap_or_else(|e| fail(&format!("ls {path}: {e}")))
            {
                let suffix = if e.kind == InodeKind::Dir { "/" } else { "" };
                println!("{:>10}  {}{}", e.size, e.name, suffix);
            }
        }
        Some("cat") => {
            let img = args.get(1).unwrap_or_else(|| usage());
            let path = args.get(2).unwrap_or_else(|| usage());
            let mut fs = open_fs(img, false);
            let data = fs
                .read("/", path)
                .unwrap_or_else(|e| fail(&format!("cat {path}: {e}")));
            std::io::stdout().write_all(&data).unwrap();
        }
        Some("check") => {
            let img = args.get(1).unwrap_or_else(|| usage());
            let mut fs = open_fs(img, false);
            let st = fs
                .check()
                .unwrap_or_else(|e| fail(&format!("check: {e}")));
            println!(
                "{img}: clean; gen {}, {}/{} blocks used, {}/{} inodes",
                st.generation, st.used_blocks, st.total_blocks, st.inodes_used, st.inodes_total
            );
        }
        _ => usage(),
    }
}
