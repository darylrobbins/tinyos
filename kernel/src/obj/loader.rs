//! Static ELF64 loader + process spawn. Parses the app image, maps its
//! PT_LOAD segments W^X into a fresh address space, sets up a stack, and
//! hands the process its bootstrap channel + record. ~one page of code, no
//! external ELF dependency (see the design doc for why fixed-base ET_EXEC).

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::arch::paging::{AddrSpace, MapFlags, sync_dcache, sync_icache};
use crate::mem::frames::{FRAME_SIZE, alloc_frames};

use super::channel::{self, ChannelEnd, Message};
use super::handle::{Handle, RIGHTS_ALL};
use super::process::Process;
use super::syscall::ABI_VERSION;
use super::Object;

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

const USER_STACK_SIZE: usize = 64 * 1024;

pub enum LoadError {
    NotElf,
    WrongArch,
    BadAbi(u32),
    NoMemory,
    BadImage,
}

impl LoadError {
    pub fn msg(&self) -> alloc::string::String {
        match self {
            LoadError::NotElf => "not an ELF64 executable".into(),
            LoadError::WrongArch => "wrong architecture (need aarch64)".into(),
            LoadError::BadAbi(v) => alloc::format!("ABI version {v} != {ABI_VERSION}"),
            LoadError::NoMemory => "out of memory".into(),
            LoadError::BadImage => "malformed image".into(),
        }
    }
}

fn rd<const N: usize>(b: &[u8], o: usize) -> Option<[u8; N]> {
    b.get(o..o + N)?.try_into().ok()
}
fn u16at(b: &[u8], o: usize) -> Option<u16> {
    rd::<2>(b, o).map(u16::from_le_bytes)
}
fn u32at(b: &[u8], o: usize) -> Option<u32> {
    rd::<4>(b, o).map(u32::from_le_bytes)
}
fn u64at(b: &[u8], o: usize) -> Option<u64> {
    rd::<8>(b, o).map(u64::from_le_bytes)
}

/// File bytes of the lowest PT_LOAD segment (where link.ld places the
/// `.tinyos_abi` stamp: u32 abi_version, then optionally u32 caps_len +
/// caps bytes emitted by the SDK's `declare_caps!`).
fn abi_blob(elf: &[u8]) -> Option<&[u8]> {
    let phoff = u64at(elf, 32)? as usize;
    let phentsize = u16at(elf, 54)? as usize;
    let phnum = u16at(elf, 56)? as usize;
    let (_, ph) = (0..phnum)
        .filter_map(|i| {
            let ph = phoff + i * phentsize;
            (u32at(elf, ph) == Some(PT_LOAD)).then_some((u64at(elf, ph + 16)?, ph))
        })
        .min_by_key(|(vaddr, _)| *vaddr)?;
    let off = u64at(elf, ph + 8)? as usize;
    let filesz = u64at(elf, ph + 32)? as usize;
    elf.get(off..(off + filesz).min(elf.len()))
}

/// Sanity cap on the caps blob; anything larger is treated as absent.
const MAX_CAPS_LEN: usize = 512;

/// Capabilities an app declares in its `.tinyos_abi` stamp. Declarations are
/// requests: each spawner intersects them with its own policy. An app with no
/// caps blob (old SDK, or none declared) gets the legacy default so existing
/// binaries keep running; an app that declares caps gets exactly those.
pub struct Manifest {
    pub console: bool,
    pub window: bool,
    pub proc: bool,
    /// Advisory: kill authority is always the spawner's call.
    pub proc_kill: bool,
    /// Requested FS subtrees; `"self"` means a spawner-chosen private dir.
    pub fs: Vec<String>,
}

impl Manifest {
    fn legacy() -> Self {
        Manifest {
            console: true,
            window: true,
            proc: true,
            proc_kill: false,
            fs: alloc::vec![String::from("self")],
        }
    }
}

/// Parse the app's declared caps, or the legacy default if none.
pub fn manifest(elf: &[u8]) -> Manifest {
    let Some(blob) = abi_blob(elf) else {
        return Manifest::legacy();
    };
    let caps = u32at(blob, 4)
        .map(|l| l as usize)
        .filter(|&l| l > 0 && l <= MAX_CAPS_LEN)
        .and_then(|l| blob.get(8..8 + l))
        .and_then(|b| core::str::from_utf8(b).ok());
    let Some(caps) = caps else {
        return Manifest::legacy();
    };
    // Declared caps → default-deny: only what's listed.
    let mut m = Manifest { console: false, window: false, proc: false, proc_kill: false, fs: Vec::new() };
    for tok in caps.lines().map(str::trim).filter(|t| !t.is_empty()) {
        match tok {
            "console" => m.console = true,
            "window" => m.window = true,
            "proc" => m.proc = true,
            "proc.kill" => {
                m.proc = true;
                m.proc_kill = true;
            }
            t => {
                if let Some(path) = t.strip_prefix("fs:") {
                    m.fs.push(String::from(path));
                }
                // Unknown tokens are ignored (they can only under-grant).
            }
        }
    }
    m
}

struct Segment {
    off: usize,
    vaddr: u64,
    filesz: usize,
    memsz: usize,
    exec: bool,
    write: bool,
}

/// Parse + map the image into `aspace`. Returns the entry point VA.
///
/// Segments may share a page (e.g. .rodata's tail and .bss's head), so the
/// whole image is one contiguous frame block and permissions accumulate
/// per page — a shared RO/RW page ends up writable (standard for merged
/// ELF segments), which the fixed page granularity makes unavoidable.
fn load_image(elf: &[u8], aspace: &mut AddrSpace, abi_expected: u32) -> Result<u64, LoadError> {
    // ELF header.
    if elf.get(0..4) != Some(&[0x7F, b'E', b'L', b'F']) {
        return Err(LoadError::NotElf);
    }
    if elf.get(4) != Some(&2) {
        return Err(LoadError::NotElf); // ELFCLASS64
    }
    if u16at(elf, 18) != Some(0xB7) {
        return Err(LoadError::WrongArch); // EM_AARCH64
    }
    let entry = u64at(elf, 24).ok_or(LoadError::BadImage)?;
    let phoff = u64at(elf, 32).ok_or(LoadError::BadImage)? as usize;
    let phentsize = u16at(elf, 54).ok_or(LoadError::BadImage)? as usize;
    let phnum = u16at(elf, 56).ok_or(LoadError::BadImage)? as usize;

    // The .tinyos_abi stamp is placed first at the image base by link.ld, so
    // it is the head of the lowest PT_LOAD segment's file data.
    if let Some(v) = abi_blob(elf).and_then(|b| u32at(b, 0)) {
        if v != abi_expected {
            return Err(LoadError::BadAbi(v));
        }
    }

    // Collect PT_LOAD segments.
    let mut segs = Vec::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        if u32at(elf, ph) != Some(PT_LOAD) {
            continue;
        }
        let flags = u32at(elf, ph + 4).ok_or(LoadError::BadImage)?;
        let memsz = u64at(elf, ph + 40).ok_or(LoadError::BadImage)? as usize;
        if memsz == 0 {
            continue;
        }
        segs.push(Segment {
            off: u64at(elf, ph + 8).ok_or(LoadError::BadImage)? as usize,
            vaddr: u64at(elf, ph + 16).ok_or(LoadError::BadImage)?,
            filesz: u64at(elf, ph + 32).ok_or(LoadError::BadImage)? as usize,
            memsz,
            exec: flags & PF_X != 0,
            write: flags & PF_W != 0,
        });
    }
    if segs.is_empty() {
        return Err(LoadError::BadImage);
    }

    // One contiguous frame block covering [base_page, end_page).
    let fs = FRAME_SIZE as u64;
    let base_page = segs.iter().map(|s| s.vaddr & !(fs - 1)).min().unwrap();
    let end_page = segs
        .iter()
        .map(|s| (s.vaddr + s.memsz as u64 + fs - 1) & !(fs - 1))
        .max()
        .unwrap();
    let pages = ((end_page - base_page) / fs) as usize;
    let pa = alloc_frames(pages).ok_or(LoadError::NoMemory)?; // zeroed => .bss

    // Copy file-backed bytes and accumulate per-page permissions.
    let mut page_exec = alloc::vec![false; pages];
    let mut page_write = alloc::vec![false; pages];
    for s in &segs {
        let dst = pa + (s.vaddr - base_page) as usize;
        let avail = s.filesz.min(elf.len().saturating_sub(s.off));
        if let Some(src) = elf.get(s.off..s.off + avail) {
            unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut u8, src.len()) };
        }
        let first = ((s.vaddr & !(fs - 1)) - base_page) / fs;
        let last = ((s.vaddr + s.memsz as u64 - 1) - base_page) / fs;
        for p in first..=last {
            page_exec[p as usize] |= s.exec;
            page_write[p as usize] |= s.write;
        }
    }

    // Visibility to a user thread on another core, then map each page with
    // its accumulated permissions (W^X preserved except on shared pages).
    sync_dcache(pa, pages * FRAME_SIZE);
    sync_icache(pa, pages * FRAME_SIZE);
    for p in 0..pages {
        aspace
            .map_page(
                base_page + (p * FRAME_SIZE) as u64,
                pa + p * FRAME_SIZE,
                MapFlags { write: page_write[p], exec: page_exec[p] },
            )
            .ok_or(LoadError::NoMemory)?;
    }
    aspace.own_block(pa, pages);

    Ok(entry)
}

/// The kernel's end of an app's console channel, plus the app's process.
pub struct SpawnedApp {
    pub process: Arc<Process>,
    pub thread_id: u32,
    pub console: Arc<ChannelEnd>,
    pub shell: Arc<ChannelEnd>,
    pub fs: Arc<ChannelEnd>,
    pub proc: Arc<ChannelEnd>,
}

/// Bootstrap grant tags (also known to the SDK).
pub use abi::bootstrap::{TAG_CONSOLE, TAG_FS, TAG_PROC, TAG_SHELL};

/// Build the bootstrap record: abi, argv, grant tags. Handles ride the msg.
fn bootstrap_record(argv: &[String], tags: &[u32]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(ABI_VERSION as u32).to_le_bytes());
    b.extend_from_slice(&(argv.len() as u32).to_le_bytes());
    for a in argv {
        b.extend_from_slice(&(a.len() as u32).to_le_bytes());
        b.extend_from_slice(a.as_bytes());
    }
    b.extend_from_slice(&(tags.len() as u32).to_le_bytes());
    for t in tags {
        b.extend_from_slice(&t.to_le_bytes());
    }
    b
}

/// Spawn `elf` as a process with explicit grants: (tag, handle) pairs
/// delivered in the bootstrap record, capability-style. Returns the process,
/// its main thread id, and the parent-side end of the app's main channel
/// (dropping it flips the app's handle 1 to PEER_CLOSED).
pub fn spawn_with_grants(
    name: String,
    elf: &[u8],
    argv: &[String],
    grants: Vec<(u32, Handle)>,
) -> Result<(Arc<Process>, u32, Arc<ChannelEnd>), LoadError> {
    let mut aspace = AddrSpace::new().ok_or(LoadError::WrongArch)?;
    let entry = load_image(elf, &mut aspace, ABI_VERSION as u32)?;

    // User stack.
    let stack_pa = alloc_frames(USER_STACK_SIZE / FRAME_SIZE).ok_or(LoadError::NoMemory)?;
    let stack_va = aspace.alloc_va(USER_STACK_SIZE);
    aspace
        .map(stack_va, stack_pa, USER_STACK_SIZE, MapFlags { write: true, exec: false }, true)
        .ok_or(LoadError::NoMemory)?;
    let sp = stack_va + USER_STACK_SIZE as u64;

    let process = Process::new(name.clone(), aspace);
    // Charge the image + stack against the process's memory quota (kernel-
    // controlled sizes; no failure path needed — the quota gates future
    // memobj_create calls).
    process.charge(process.aspace.lock().mapped_bytes());

    // Handle 1 = app's end of the main channel (installed first).
    let (main_app, main_kern) = channel::create();
    {
        let mut t = process.handles.lock();
        t.insert(Handle::new(Object::Channel(main_app), RIGHTS_ALL))
            .map_err(|_| LoadError::NoMemory)?;
    }

    // Bootstrap message: record bytes + the granted handles riding along.
    let tags: Vec<u32> = grants.iter().map(|(t, _)| *t).collect();
    let record = bootstrap_record(argv, &tags);
    let handles: Vec<Handle> = grants.into_iter().map(|(_, h)| h).collect();
    main_kern
        .send(Message { bytes: record, handles })
        .map_err(|_| LoadError::NoMemory)?;

    let aspace_arc = process.aspace.clone();
    let thread_id = crate::sched::spawn_user(
        name,
        crate::sched::thread::Class::Normal,
        if crate::sched::online_cpus() > 1 { 0b1110 } else { 0b0001 },
        aspace_arc,
        entry,
        sp,
        0,
        Some(process.clone()),
    );
    process
        .main_thread
        .store(thread_id, core::sync::atomic::Ordering::Relaxed);

    Ok((process, thread_id, main_kern))
}

/// Which bootstrap channels a spawner actually grants: the app's declared
/// manifest intersected with the spawner's policy. The kernel ends always
/// exist (channels are cheap); an ungranted app end is simply dropped, so
/// the app never receives that tag and the kernel end just reads PEER_CLOSED.
pub struct GrantSet {
    pub console: bool,
    pub window: bool,
    pub fs: bool,
    pub proc: bool,
}

impl GrantSet {
    /// Developer mode (terminal `run`, tests): everything.
    pub fn all() -> Self {
        GrantSet { console: true, window: true, fs: true, proc: true }
    }
}

/// Load `elf`, spawn it as a user process named `name` with `argv`, granting
/// the channels selected by `grants`. Returns the kernel-side ends to pump.
pub fn spawn(
    name: String,
    elf: &[u8],
    argv: &[String],
    grant_set: &GrantSet,
) -> Result<SpawnedApp, LoadError> {
    let (console_app, console_kern) = channel::create();
    let (shell_app, shell_kern) = channel::create();
    let (fs_app, fs_kern) = channel::create();
    let (proc_app, proc_kern) = channel::create();
    let mut grants = Vec::new();
    for (on, tag, end) in [
        (grant_set.console, TAG_CONSOLE, console_app),
        (grant_set.window, TAG_SHELL, shell_app),
        (grant_set.fs, TAG_FS, fs_app),
        (grant_set.proc, TAG_PROC, proc_app),
    ] {
        if on {
            grants.push((tag, Handle::new(Object::Channel(end), RIGHTS_ALL)));
        }
    }
    let (process, thread_id, main_kern) = spawn_with_grants(name, elf, argv, grants)?;
    // Park the kernel's bootstrap end in the process so it lives as long as
    // the app does — dropping it would flip the app's main channel to
    // PEER_CLOSED. The kernel never sends on it again.
    process.keep.lock().push(main_kern);
    Ok(SpawnedApp {
        process,
        thread_id,
        console: console_kern,
        shell: shell_kern,
        fs: fs_kern,
        proc: proc_kern,
    })
}
