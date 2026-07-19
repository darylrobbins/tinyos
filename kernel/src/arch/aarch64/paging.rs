//! Per-process user address spaces in TTBR1.
//!
//! The kernel keeps UEFI's identity map in TTBR0 (EL1-only pages — EL0 can't
//! touch kernel memory). Each process owns a TTBR1 tree covering a 16 GiB
//! window at USER_BASE (TCR.T1SZ=30): one L1 root (16 used entries, 1 GiB
//! each) -> L2 (2 MiB) -> L3 (4 KiB pages). User pages are nG with an 8-bit
//! ASID in TTBR1[63:48], so switching spaces is `msr ttbr1_el1; isb` with no
//! TLB flush; a full `tlbi aside1is` happens only at teardown.

use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU16, Ordering};

use crate::mem::frames::{FRAME_SIZE, alloc_frames, free_frames};

/// Bottom of the TTBR1 region with T1SZ=30 (bits 63:34 all ones).
pub const USER_BASE: u64 = 0xFFFF_FFFC_0000_0000;
pub const USER_SIZE: u64 = 1 << 34;
/// Fixed link base for app images (4 MiB into the window).
pub const APP_IMAGE_BASE: u64 = USER_BASE + 0x40_0000;
/// Kernel-chosen mappings (stacks, memobjs) bump upward from 8 GiB in.
const DYN_BASE: u64 = USER_BASE + (1 << 33);

const DESC_VALID_TABLE: u64 = 0b11;
const DESC_VALID_PAGE: u64 = 0b11;
const ATTR_AF: u64 = 1 << 10;
const ATTR_SH_INNER: u64 = 0b11 << 8;
const ATTR_NG: u64 = 1 << 11;
const ATTR_PXN: u64 = 1 << 53;
const ATTR_UXN: u64 = 1 << 54;
const AP_EL0_RW: u64 = 0b01 << 6;
const AP_EL0_RO: u64 = 0b11 << 6;
const PA_MASK: u64 = 0x0000_FFFF_FFFF_F000;

#[derive(Clone, Copy, PartialEq)]
pub struct MapFlags {
    pub write: bool,
    pub exec: bool,
}

struct Mapping {
    va: u64,
    len: u64,
    pa: usize,
    flags: MapFlags,
}

/// A user address space: TTBR1 tree + ASID + bookkeeping. Not `Sync`; the
/// owner (Process) wraps it in a Mutex.
pub struct AddrSpace {
    root: usize, // PA of the L1 table
    asid: u16,
    tables: Vec<usize>,       // PAs of L2/L3 tables (freed on drop)
    owned: Vec<(usize, usize)>, // (PA, page_count) blocks freed on drop
    maps: Vec<Mapping>,       // for user-pointer validation
    bump: u64,
    /// Reclaimed VA ranges (start, reserved_len) available for reuse.
    va_free: Vec<(u64, u64)>,
}

static NEXT_ASID: AtomicU16 = AtomicU16::new(1);

fn alloc_asid() -> u16 {
    let asid = NEXT_ASID.fetch_add(1, Ordering::Relaxed) & 0xFF;
    if asid == 0 {
        // ASID wrap: nuke everything so recycled ASIDs start clean.
        unsafe { asm!("dsb ishst", "tlbi vmalle1is", "dsb ish", "isb") };
        return alloc_asid();
    }
    asid
}

impl AddrSpace {
    pub fn new() -> Option<Self> {
        Some(Self {
            root: alloc_frames(1)?,
            asid: alloc_asid(),
            tables: Vec::new(),
            owned: Vec::new(),
            maps: Vec::new(),
            bump: DYN_BASE,
            va_free: Vec::new(),
        })
    }

    /// Value for TTBR1_EL1: root address + ASID.
    pub fn ttbr1(&self) -> u64 {
        (self.root as u64) | ((self.asid as u64) << 48)
    }

    pub fn asid(&self) -> u16 {
        self.asid
    }

    fn table(pa: usize) -> *mut u64 {
        pa as *mut u64 // identity map: PA is directly addressable
    }

    /// Walk to the L3 entry for `va`, creating tables as needed.
    fn entry(&mut self, va: u64) -> Option<*mut u64> {
        // The user window ends exactly at 2^64, so `USER_BASE + USER_SIZE`
        // overflows; compare via the offset instead.
        debug_assert!(va >= USER_BASE && va - USER_BASE < USER_SIZE);
        let l1i = ((va >> 30) & 0xF) as usize;
        let l2i = ((va >> 21) & 0x1FF) as usize;
        let l3i = ((va >> 12) & 0x1FF) as usize;

        let mut tbl = self.root;
        for idx in [l1i, l2i] {
            let slot = unsafe { Self::table(tbl).add(idx) };
            let mut desc = unsafe { slot.read_volatile() };
            if desc & 1 == 0 {
                let new = alloc_frames(1)?;
                self.tables.push(new);
                desc = new as u64 | DESC_VALID_TABLE;
                unsafe { slot.write_volatile(desc) };
            }
            tbl = (desc & PA_MASK) as usize;
        }
        Some(unsafe { Self::table(tbl).add(l3i) })
    }

    fn page_desc(pa: usize, flags: MapFlags) -> u64 {
        let ap = if flags.write { AP_EL0_RW } else { AP_EL0_RO };
        let uxn = if flags.exec { 0 } else { ATTR_UXN };
        pa as u64
            | DESC_VALID_PAGE
            | ((normal_attr_idx() as u64) << 2)
            | ATTR_AF
            | ATTR_SH_INNER
            | ATTR_NG
            | ATTR_PXN
            | ap
            | uxn
    }

    /// Map `len` bytes (frame-aligned) of physical memory at `va`. If `own`,
    /// the frames are freed when the space drops (image/stack pages; memobj
    /// frames stay owned by their MemObj). A page already mapped is
    /// re-pointed — callers must avoid double-owning the same frame.
    pub fn map(&mut self, va: u64, pa: usize, len: usize, flags: MapFlags, own: bool) -> Option<()> {
        debug_assert_eq!(va % FRAME_SIZE as u64, 0);
        debug_assert_eq!(pa % FRAME_SIZE, 0);
        let pages = len.div_ceil(FRAME_SIZE);
        for i in 0..pages {
            let e = self.entry(va + (i * FRAME_SIZE) as u64)?;
            unsafe { e.write_volatile(Self::page_desc(pa + i * FRAME_SIZE, flags)) };
        }
        if own {
            self.owned.push((pa, pages));
        }
        self.maps.push(Mapping { va, len: (pages * FRAME_SIZE) as u64, pa, flags });
        unsafe { asm!("dsb ishst") };
        Some(())
    }

    /// Map a single page with its own flags (loader use: image pages sharing
    /// a frame get per-page permissions). Ownership of the backing block is
    /// registered once via `own_block`.
    pub fn map_page(&mut self, va: u64, pa: usize, flags: MapFlags) -> Option<()> {
        let e = self.entry(va)?;
        unsafe { e.write_volatile(Self::page_desc(pa, flags)) };
        self.maps.push(Mapping { va, len: FRAME_SIZE as u64, pa, flags });
        unsafe { asm!("dsb ishst") };
        Some(())
    }

    /// Register a contiguous frame block to be freed when the space drops.
    pub fn own_block(&mut self, pa: usize, pages: usize) {
        self.owned.push((pa, pages));
    }

    /// Total bytes currently mapped (image, stack, memobjs).
    pub fn mapped_bytes(&self) -> usize {
        self.maps.iter().map(|m| m.len as usize).sum()
    }

    /// Reserve a kernel-chosen VA range (page-aligned) for `len` bytes.
    /// Reclaimed ranges are reused first-fit; otherwise the bump grows.
    pub fn alloc_va(&mut self, len: usize) -> u64 {
        let pages = len.div_ceil(FRAME_SIZE) as u64;
        let need = (pages + 1) * FRAME_SIZE as u64; // +1 guard page
        if let Some(i) = self.va_free.iter().position(|(_, l)| *l >= need) {
            let (start, avail) = self.va_free[i];
            if avail == need {
                self.va_free.swap_remove(i);
            } else {
                self.va_free[i] = (start + need, avail - need);
            }
            return start;
        }
        let va = self.bump;
        self.bump += need;
        va
    }

    /// Unmap the memobj-backed mapping starting at `va`: clear its PTEs,
    /// flush per page, drop the record, and recycle the VA range. Refuses
    /// image/stack pages (frames owned by the address space). Returns the
    /// mapping's (pa, len).
    pub fn unmap(&mut self, va: u64) -> Option<(usize, u64)> {
        let idx = self.maps.iter().position(|m| m.va == va)?;
        let (pa, len) = (self.maps[idx].pa, self.maps[idx].len);
        if self
            .owned
            .iter()
            .any(|(opa, pages)| pa >= *opa && pa < opa + pages * FRAME_SIZE)
        {
            return None; // image/stack: dies with the address space only
        }
        let pages = len / FRAME_SIZE as u64;
        for i in 0..pages {
            let page_va = va + i * FRAME_SIZE as u64;
            if let Some(e) = self.entry(page_va) {
                unsafe { e.write_volatile(0) };
                tlbi_page(self.asid, page_va);
            }
        }
        unsafe { asm!("dsb ish", "isb") };
        self.maps.swap_remove(idx);
        self.va_free.push((va, len + FRAME_SIZE as u64)); // incl. guard page
        Some((pa, len))
    }

    /// Does any current mapping reference the physical range [base, base+len)?
    pub fn references_pa_range(&self, base: usize, len: usize) -> bool {
        self.maps
            .iter()
            .any(|m| m.pa < base + len && base < m.pa + m.len as usize)
    }

    /// Tighten permissions on an existing mapping (e.g. make code RX->R).
    pub fn protect(&mut self, va: u64, len: usize, flags: MapFlags) {
        let pages = len.div_ceil(FRAME_SIZE);
        for i in 0..pages {
            let page_va = va + (i * FRAME_SIZE) as u64;
            if let Some(e) = self.entry(page_va) {
                let old = unsafe { e.read_volatile() };
                if old & 1 != 0 {
                    let pa = (old & PA_MASK) as usize;
                    unsafe { e.write_volatile(Self::page_desc(pa, flags)) };
                    tlbi_page(self.asid, page_va);
                }
            }
        }
        for m in &mut self.maps {
            if m.va == va {
                m.flags = flags;
            }
        }
        unsafe { asm!("dsb ish", "isb") };
    }

    /// Validate a user buffer: fully inside recorded mappings, with write
    /// permission if `write`.
    pub fn user_buf_ok(&self, va: u64, len: u64, write: bool) -> bool {
        if len == 0 {
            return true;
        }
        let end = match va.checked_add(len) {
            Some(e) => e,
            None => return false,
        };
        // A buffer may span adjacent mappings; walk forward greedily.
        let mut at = va;
        while at < end {
            match self
                .maps
                .iter()
                .find(|m| m.va <= at && at < m.va + m.len && (!write || m.flags.write))
            {
                Some(m) => at = m.va + m.len,
                None => return false,
            }
        }
        true
    }
}

impl Drop for AddrSpace {
    fn drop(&mut self) {
        // Flush every nG entry for this ASID, then free the tree.
        unsafe {
            asm!("dsb ishst", "tlbi aside1is, {0}", "dsb ish", "isb",
                 in(reg) (self.asid as u64) << 48);
        }
        for &(pa, pages) in &self.owned {
            unsafe { free_frames(pa, pages) };
        }
        for &t in &self.tables {
            unsafe { free_frames(t, 1) };
        }
        unsafe { free_frames(self.root, 1) };
    }
}

/// Clean the dcache to point-of-coherency over a range so bytes a kernel
/// thread wrote (through the identity map) are visible to a user thread that
/// may run on another core. Needed for every loaded data page; code pages
/// additionally go through `sync_icache`.
pub fn sync_dcache(pa: usize, len: usize) {
    let mut a = pa & !63;
    while a < pa + len {
        unsafe { asm!("dc cvac, {0}", in(reg) a) };
        a += 64;
    }
    unsafe { asm!("dsb ish") };
}

/// Make freshly written code visible to instruction fetch: clean dcache to
/// PoU and invalidate icache for the range. Call after writing user code
/// pages (via their identity-mapped PA) and before any EL0 execution.
pub fn sync_icache(pa: usize, len: usize) {
    let mut a = pa & !63;
    while a < pa + len {
        unsafe { asm!("dc cvau, {0}", in(reg) a) };
        a += 64;
    }
    unsafe { asm!("dsb ish") };
    let mut a = pa & !63;
    while a < pa + len {
        unsafe { asm!("ic ivau, {0}", in(reg) a) };
        a += 64;
    }
    unsafe { asm!("dsb ish", "isb") };
}

fn tlbi_page(asid: u16, va: u64) {
    let arg = ((asid as u64) << 48) | ((va >> 12) & 0xFFF_FFFF_FFFF);
    unsafe { asm!("dsb ishst", "tlbi vae1is, {0}", in(reg) arg) };
}

// ---------------------------------------------------------------------------
// CPU configuration
// ---------------------------------------------------------------------------

/// Empty L1 table used as TTBR1 when no process is active (ASID 0).
#[repr(C, align(4096))]
struct NullTable([u8; 4096]);
static NULL_L1: NullTable = NullTable([0; 4096]);

static NORMAL_ATTR_IDX: AtomicU16 = AtomicU16::new(0xFFFF);

/// MAIR index for Normal WB-WA memory (0xFF), located or installed by
/// `init_cpu` on the BSP.
fn normal_attr_idx() -> u8 {
    NORMAL_ATTR_IDX.load(Ordering::Relaxed) as u8
}

pub fn null_ttbr1() -> u64 {
    &raw const NULL_L1 as u64 // ASID 0
}

/// Enable TTBR1 user translation on the calling CPU. BSP calls this once in
/// kmain (before the scheduler); APs inherit TCR/SCTLR via ApBoot's live
/// register snapshot but still need their TTBR1 pointed at the null table.
pub fn init_cpu() {
    unsafe {
        // Locate (or install) a Normal WB-WA attribute in MAIR.
        if NORMAL_ATTR_IDX.load(Ordering::Relaxed) == 0xFFFF {
            let mut mair: u64;
            asm!("mrs {0}, mair_el1", out(reg) mair);
            let idx = (0..8).find(|i| (mair >> (i * 8)) & 0xFF == 0xFF);
            let idx = match idx {
                Some(i) => i,
                None => {
                    // Claim index 7 (unused by edk2). No live TLB entries
                    // reference it, so a plain write + isb is sufficient.
                    mair |= 0xFF << 56;
                    asm!("msr mair_el1, {0}", "isb", in(reg) mair);
                    7
                }
            };
            NORMAL_ATTR_IDX.store(idx as u16, Ordering::Relaxed);
        }

        // Point TTBR1 at the empty table before enabling walks.
        asm!("msr ttbr1_el1, {0}", "isb", in(reg) null_ttbr1());

        // TCR: T1SZ=30, TG1=4K, SH1=inner, ORGN1/IRGN1=WBWA, EPD1=0, A1=1
        // (ASID from TTBR1).
        let mut tcr: u64;
        asm!("mrs {0}, tcr_el1", out(reg) tcr);
        tcr &= !((0x3F << 16) | (1 << 23) | (0b11 << 30) | (0b11 << 28) | (0b11 << 26) | (0b11 << 24));
        tcr |= 30 << 16; // T1SZ
        tcr |= 0b10 << 30; // TG1 = 4K
        tcr |= 0b11 << 28; // SH1 inner shareable
        tcr |= 0b01 << 26; // ORGN1 WBWA
        tcr |= 0b01 << 24; // IRGN1 WBWA
        tcr |= 1 << 22; // A1: ASID from TTBR1
        asm!("msr tcr_el1, {0}", "isb", in(reg) tcr);

        // SCTLR: SPAN=1 (leave PAN clear so syscalls can touch user pages),
        // UCI/UCT/DZE=1 (EL0 cache ops, CTR_EL0, DC ZVA).
        let mut sctlr: u64;
        asm!("mrs {0}, sctlr_el1", out(reg) sctlr);
        sctlr |= (1 << 23) | (1 << 26) | (1 << 15) | (1 << 14);
        asm!("msr sctlr_el1, {0}", "isb", in(reg) sctlr);

        // No stale TTBR1 walks can exist (EPD1 was set), but start clean.
        asm!("dsb ishst", "tlbi vmalle1", "dsb ish", "isb");
    }
}

/// Boot-time smoke test: map a frame in a fresh space, write through the
/// user VA from EL1, read it back, and check the physical frame saw it.
pub fn self_test() -> bool {
    let mut space = match AddrSpace::new() {
        Some(s) => s,
        None => return false,
    };
    let frame = match alloc_frames(1) {
        Some(f) => f,
        None => return false,
    };
    let va = APP_IMAGE_BASE;
    if space
        .map(va, frame, FRAME_SIZE, MapFlags { write: true, exec: false }, true)
        .is_none()
    {
        return false;
    }
    unsafe {
        asm!("msr ttbr1_el1, {0}", "isb", in(reg) space.ttbr1());
        let p = va as *mut u64;
        p.write_volatile(0xDEAD_BEEF_CAFE_F00D);
        let via_va = p.read_volatile();
        let via_pa = (frame as *const u64).read_volatile();
        asm!("msr ttbr1_el1, {0}", "isb", in(reg) null_ttbr1());
        via_va == 0xDEAD_BEEF_CAFE_F00D && via_pa == 0xDEAD_BEEF_CAFE_F00D
    }
}
