//! Modern virtio-blk-pci (device 0x1042), synchronous and polled: one
//! request in flight, 3-descriptor chains (header | data | status). QEMU's
//! `-drive format=raw,file=fat:rw:esp` attaches the synthesized vvfat disk
//! behind exactly this device on the virt board.

use alloc::alloc::{Layout, alloc_zeroed};

use super::mmio::{r8, r16, r32, w8, w16, w32, w64};
use super::pci::{BarAllocator, PciDevice, VENDOR_VIRTIO, scan};

pub const SECTOR: usize = 512;
const DEV_BLK_MODERN: u16 = 0x1042;
/// Transitional virtio-blk (QEMU default on a root bus); still exposes the
/// modern vendor capabilities + VERSION_1, which is all we use.
const DEV_BLK_TRANSITIONAL: u16 = 0x1001;

const CAP_VENDOR: u8 = 0x09;
const CFG_COMMON: u8 = 1;
const CFG_NOTIFY: u8 = 2;

const DEVICE_FEATURE_SELECT: usize = 0;
const DEVICE_FEATURE: usize = 4;
const DRIVER_FEATURE_SELECT: usize = 8;
const DRIVER_FEATURE: usize = 12;
const DEVICE_STATUS: usize = 20;
const QUEUE_SELECT: usize = 22;
const QUEUE_SIZE: usize = 24;
const QUEUE_ENABLE: usize = 28;
const QUEUE_NOTIFY_OFF: usize = 30;
const QUEUE_DESC: usize = 32;
const QUEUE_DRIVER: usize = 40;
const QUEUE_DEVICE: usize = 48;

const STATUS_ACK: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;
const FEATURE_VERSION_1: u32 = 1; // bit 32 -> select-word 1, bit 0

const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2;

/// avail.flags bit 0: ask the device to suppress completion interrupts.
const VIRTQ_AVAIL_F_NO_INTERRUPT: u16 = 1;

const REQ_T_IN: u32 = 0; // device -> memory (read)

/// Max sectors per request (data buffer size).
const MAX_SECTORS: usize = 8;

fn fence() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

pub struct VirtioBlk {
    notify_addr: usize,
    desc: usize,
    avail: usize,
    used: usize,
    header: usize, // 16-byte request header
    data: usize,   // MAX_SECTORS * SECTOR
    status: usize, // 1 byte
    last_used: u16,
    avail_idx: u16,
    pub capacity_sectors: u64,
}

impl VirtioBlk {
    /// Find and initialize the first modern virtio-blk device.
    pub fn init() -> Option<Self> {
        let pci = scan().find(|d| {
            d.vendor == VENDOR_VIRTIO
                && matches!(d.device, DEV_BLK_MODERN | DEV_BLK_TRANSITIONAL)
        })?;
        kprintln!("tinyos: virtio-blk at bdf={:#x} dev={:#06x}", pci.bdf, pci.device);
        let mut alloc = BarAllocator::new();
        Self::setup(&pci, &mut alloc)
    }

    fn setup(pci: &PciDevice, alloc: &mut BarAllocator) -> Option<Self> {
        pci.enable();

        let mut common = None;
        let mut notify = None;
        let mut device_cfg = None;
        let mut cap_ptr = (pci.read8(0x34) & !3) as usize;
        while cap_ptr != 0 {
            if pci.read8(cap_ptr) == CAP_VENDOR {
                let cfg_type = pci.read8(cap_ptr + 3);
                let bar = pci.read8(cap_ptr + 4) as usize;
                let offset = pci.read32(cap_ptr + 8) as usize;
                match cfg_type {
                    CFG_COMMON => common = Some((bar, offset)),
                    CFG_NOTIFY => {
                        let mult = pci.read32(cap_ptr + 16) as usize;
                        notify = Some((bar, offset, mult));
                    }
                    4 => device_cfg = Some((bar, offset)), // CFG_DEVICE
                    _ => {}
                }
            }
            cap_ptr = (pci.read8(cap_ptr + 1) & !3) as usize;
        }
        let (cbar, coff) = common?;
        let (nbar, noff, nmult) = notify?;
        let common = pci.bar_addr(cbar, alloc)? as usize + coff;
        let notify_base = pci.bar_addr(nbar, alloc)? as usize + noff;

        w8(common + DEVICE_STATUS, 0);
        while r8(common + DEVICE_STATUS) != 0 {}
        w8(common + DEVICE_STATUS, STATUS_ACK);
        w8(common + DEVICE_STATUS, STATUS_ACK | STATUS_DRIVER);

        w32(common + DEVICE_FEATURE_SELECT, 1);
        if r32(common + DEVICE_FEATURE) & FEATURE_VERSION_1 == 0 {
            return None; // legacy-only device
        }
        w32(common + DRIVER_FEATURE_SELECT, 0);
        w32(common + DRIVER_FEATURE, 0);
        w32(common + DRIVER_FEATURE_SELECT, 1);
        w32(common + DRIVER_FEATURE, FEATURE_VERSION_1);
        w8(common + DEVICE_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
        if r8(common + DEVICE_STATUS) & STATUS_FEATURES_OK == 0 {
            return None;
        }

        // Queue 0: three descriptors are all a synchronous driver needs.
        w16(common + QUEUE_SELECT, 0);
        let qsize = r16(common + QUEUE_SIZE).min(8);
        w16(common + QUEUE_SIZE, qsize);
        let n = qsize as usize;

        let desc_bytes = (n * 16 + 4095) & !4095;
        let avail_bytes = (6 + 2 * n + 4095) & !4095;
        let used_bytes = (6 + 8 * n + 4095) & !4095;
        let data_bytes = 16 + MAX_SECTORS * SECTOR + 16; // header + data + status (padded)
        let layout =
            Layout::from_size_align(desc_bytes + avail_bytes + used_bytes + data_bytes, 4096)
                .unwrap();
        let mem = unsafe { alloc_zeroed(layout) } as usize;
        let desc = mem;
        let avail = mem + desc_bytes;
        let used = avail + avail_bytes;
        let header = used + used_bytes;
        let data = header + 16;
        let status = data + MAX_SECTORS * SECTOR;

        // This driver is purely polled: never let the device raise a
        // completion interrupt. Its INTx would share a PCIe SPI with the
        // virtio-input devices, and the IRQ handler only deasserts *their*
        // ISR — an unacknowledged blk INTx would storm and wedge the CPU.
        w16(avail, VIRTQ_AVAIL_F_NO_INTERRUPT);

        w64(common + QUEUE_DESC, desc as u64);
        w64(common + QUEUE_DRIVER, avail as u64);
        w64(common + QUEUE_DEVICE, used as u64);
        w16(common + QUEUE_ENABLE, 1);
        let notify_addr = notify_base + r16(common + QUEUE_NOTIFY_OFF) as usize * nmult;

        w8(
            common + DEVICE_STATUS,
            STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );

        let capacity_sectors = device_cfg
            .and_then(|(bar, off)| {
                let base = pci.bar_addr(bar, alloc)? as usize + off;
                Some(r32(base) as u64 | (r32(base + 4) as u64) << 32)
            })
            .unwrap_or(0);

        Some(Self {
            notify_addr,
            desc,
            avail,
            used,
            header,
            data,
            status,
            last_used: 0,
            avail_idx: 0,
            capacity_sectors,
        })
    }

    /// Synchronously read `count` (≤ MAX_SECTORS) sectors at `lba` into `out`.
    fn read_chunk(&mut self, lba: u64, count: usize, out: &mut [u8]) -> bool {
        debug_assert!(count <= MAX_SECTORS && out.len() >= count * SECTOR);

        // Header: type IN, sector lba.
        w32(self.header, REQ_T_IN);
        w32(self.header + 4, 0);
        w64(self.header + 8, lba);
        w8(self.status, 0xFF);

        // Chain: 0 header (RO) -> 1 data (W) -> 2 status (W).
        let d = |i: usize| self.desc + i * 16;
        w64(d(0), self.header as u64);
        w32(d(0) + 8, 16);
        w16(d(0) + 12, DESC_NEXT);
        w16(d(0) + 14, 1);
        w64(d(1), self.data as u64);
        w32(d(1) + 8, (count * SECTOR) as u32);
        w16(d(1) + 12, DESC_NEXT | DESC_WRITE);
        w16(d(1) + 14, 2);
        w64(d(2), self.status as u64);
        w32(d(2) + 8, 1);
        w16(d(2) + 12, DESC_WRITE);
        w16(d(2) + 14, 0);

        let n = 8usize; // queue slots (>= qsize we requested)
        let slot = self.avail + 4 + (self.avail_idx as usize % n) * 2;
        w16(slot, 0);
        fence();
        self.avail_idx = self.avail_idx.wrapping_add(1);
        w16(self.avail + 2, self.avail_idx);
        fence();
        w16(self.notify_addr, 0);

        // Poll for completion (bounded ~1s).
        let t0 = crate::arch::timer::uptime_us();
        loop {
            fence();
            if r16(self.used + 2) != self.last_used {
                break;
            }
            if crate::arch::timer::uptime_us() - t0 > 1_000_000 {
                return false;
            }
            core::hint::spin_loop();
        }
        self.last_used = self.last_used.wrapping_add(1);
        if r8(self.status) != 0 {
            return false;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.data as *const u8,
                out.as_mut_ptr(),
                count * SECTOR,
            )
        };
        true
    }

    /// Read `count` sectors at `lba` into `out` (any count).
    pub fn read_sectors(&mut self, mut lba: u64, mut count: usize, out: &mut [u8]) -> bool {
        let mut off = 0;
        while count > 0 {
            let chunk = count.min(MAX_SECTORS);
            if !self.read_chunk(lba, chunk, &mut out[off..off + chunk * SECTOR]) {
                return false;
            }
            lba += chunk as u64;
            count -= chunk;
            off += chunk * SECTOR;
        }
        true
    }
}
