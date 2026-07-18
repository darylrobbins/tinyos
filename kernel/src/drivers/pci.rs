//! Minimal PCI ECAM access for the QEMU virt board. All devices sit on bus 0.

use super::mmio;

pub const VENDOR_VIRTIO: u16 = 0x1af4;

/// ECAM candidates: QEMU virt highmem + legacy low (aarch64), q35 MMCONFIG (x86).
#[cfg(target_arch = "aarch64")]
const ECAM_BASES: [usize; 2] = [0x40_1000_0000, 0x3f00_0000];
#[cfg(target_arch = "x86_64")]
const ECAM_BASES: [usize; 2] = [0xe000_0000, 0xb000_0000];

#[derive(Clone, Copy)]
pub struct PciDevice {
    ecam: usize,
    pub bdf: u32,
    pub vendor: u16,
    pub device: u16,
}

impl PciDevice {
    fn cfg(&self, off: usize) -> usize {
        self.ecam + ((self.bdf as usize) << 12) + off
    }

    pub fn read32(&self, off: usize) -> u32 {
        mmio::r32(self.cfg(off))
    }

    pub fn write32(&self, off: usize, v: u32) {
        mmio::w32(self.cfg(off), v)
    }

    pub fn read16(&self, off: usize) -> u16 {
        mmio::r16(self.cfg(off))
    }

    pub fn write16(&self, off: usize, v: u16) {
        mmio::w16(self.cfg(off), v)
    }

    pub fn read8(&self, off: usize) -> u8 {
        mmio::r8(self.cfg(off))
    }

    /// Enable memory space + bus mastering.
    pub fn enable(&self) {
        let cmd = self.read16(0x04);
        self.write16(0x04, cmd | 0x6);
    }

    /// Physical address of a memory BAR, assigning one if the firmware
    /// left it unprogrammed. Returns None for an I/O or absent BAR.
    pub fn bar_addr(&self, bar: usize, alloc: &mut BarAllocator) -> Option<u64> {
        let off = 0x10 + bar * 4;
        let lo = self.read32(off);
        if lo & 1 != 0 {
            return None; // I/O BAR
        }
        let is64 = lo & 0b110 == 0b100;
        let mut addr = (lo & !0xf) as u64;
        if is64 {
            addr |= (self.read32(off + 4) as u64) << 32;
        }
        if addr != 0 {
            return Some(addr);
        }
        // Size-probe and assign.
        self.write32(off, !0);
        let mask = self.read32(off) & !0xf;
        if mask == 0 {
            return None;
        }
        let size = (!mask).wrapping_add(1) as u64;
        let assigned = alloc.alloc(size);
        self.write32(off, assigned as u32);
        if is64 {
            self.write32(off + 4, (assigned >> 32) as u32);
        }
        Some(assigned)
    }
}

pub struct BarAllocator {
    next: u64,
}

impl BarAllocator {
    /// Top region of the virt 32-bit PCI MMIO window (0x1000_0000..0x3EFF_0000);
    /// edk2 allocates from the bottom, so the top is free.
    pub fn new() -> Self {
        #[cfg(target_arch = "aarch64")]
        return Self { next: 0x3a00_0000 };
        #[cfg(target_arch = "x86_64")]
        return Self { next: 0xc800_0000 };
    }

    fn alloc(&mut self, size: u64) -> u64 {
        let align = size.max(0x1000);
        self.next = (self.next + align - 1) & !(align - 1);
        let addr = self.next;
        self.next += size;
        addr
    }
}

/// Scan bus 0 and return all devices.
pub fn scan() -> impl Iterator<Item = PciDevice> {
    // Host bridge at 0:0.0 must have a real vendor ID; unmapped candidates
    // read as all-ones or all-zeroes.
    let ecam = ECAM_BASES
        .into_iter()
        .find(|&base| {
            let vendor = mmio::r32(base) & 0xFFFF;
            vendor != 0xFFFF && vendor != 0
        })
        .expect("no PCI ECAM found");

    (0u32..32).filter_map(move |dev| {
        let bdf = dev << 3;
        let id = mmio::r32(ecam + ((bdf as usize) << 12));
        if id == !0u32 {
            return None;
        }
        Some(PciDevice {
            ecam,
            bdf,
            vendor: id as u16,
            device: (id >> 16) as u16,
        })
    })
}
