//! Modern (non-legacy) virtio-pci transport with a single polled split
//! virtqueue. No interrupts: the used ring is polled from the event loop.

use alloc::alloc::{alloc_zeroed, Layout};

use super::mmio::{r8 as mmio_r8, r16 as mmio_r16, r32 as mmio_r32, w8 as mmio_w8, w16 as mmio_w16, w32 as mmio_w32, w64 as mmio_w64};
use super::pci::{BarAllocator, PciDevice};

const CAP_VENDOR: u8 = 0x09;
const CFG_COMMON: u8 = 1;
const CFG_NOTIFY: u8 = 2;
const CFG_ISR: u8 = 3;

// common config offsets
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

const FEATURE_VERSION_1: u32 = 1; // bit 32 -> word 1, bit 0

const DESC_WRITE: u16 = 2;


fn fence() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// A device with one device-writable queue of fixed-size buffers
/// (all virtio-input needs).
pub struct VirtioDevice {
    common: usize,
    notify_addr: usize,
    isr_addr: usize,
    queue_size: u16,
    desc: usize,
    avail: usize,
    used: usize,
    buffers: usize,
    buf_len: usize,
    last_used: u16,
    avail_idx: u16,
}

impl VirtioDevice {
    /// Initialize queue 0 with `count` device-writable buffers of `buf_len`.
    pub fn init(pci: &PciDevice, alloc: &mut BarAllocator, buf_len: usize) -> Option<Self> {
        pci.enable();

        // Walk capabilities for common + notify structures.
        let mut common = None;
        let mut notify = None; // (bar, offset, multiplier)
        let mut isr = None;
        let mut cap_ptr = (pci.read8(0x34) & !3) as usize;
        while cap_ptr != 0 {
            let id = pci.read8(cap_ptr);
            if id == CAP_VENDOR {
                let cfg_type = pci.read8(cap_ptr + 3);
                let bar = pci.read8(cap_ptr + 4) as usize;
                let offset = pci.read32(cap_ptr + 8) as usize;
                match cfg_type {
                    CFG_COMMON => common = Some((bar, offset)),
                    CFG_NOTIFY => {
                        let mult = pci.read32(cap_ptr + 16) as usize;
                        notify = Some((bar, offset, mult));
                    }
                    CFG_ISR => isr = Some((bar, offset)),
                    _ => {}
                }
            }
            cap_ptr = (pci.read8(cap_ptr + 1) & !3) as usize;
        }
        let (cbar, coff) = common?;
        let (nbar, noff, nmult) = notify?;

        let (ibar, ioff) = isr?;
        let common = pci.bar_addr(cbar, alloc)? as usize + coff;
        let notify_base = pci.bar_addr(nbar, alloc)? as usize + noff;
        let isr_addr = pci.bar_addr(ibar, alloc)? as usize + ioff;

        // Reset, then acknowledge.
        mmio_w8(common + DEVICE_STATUS, 0);
        while mmio_r8(common + DEVICE_STATUS) != 0 {}
        mmio_w8(common + DEVICE_STATUS, STATUS_ACK);
        mmio_w8(common + DEVICE_STATUS, STATUS_ACK | STATUS_DRIVER);

        // Features: VERSION_1 only.
        mmio_w32(common + DEVICE_FEATURE_SELECT, 1);
        let f1 = mmio_r32(common + DEVICE_FEATURE);
        assert!(f1 & FEATURE_VERSION_1 != 0, "legacy-only virtio device");
        mmio_w32(common + DRIVER_FEATURE_SELECT, 0);
        mmio_w32(common + DRIVER_FEATURE, 0);
        mmio_w32(common + DRIVER_FEATURE_SELECT, 1);
        mmio_w32(common + DRIVER_FEATURE, FEATURE_VERSION_1);
        mmio_w8(
            common + DEVICE_STATUS,
            STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK,
        );
        assert!(
            mmio_r8(common + DEVICE_STATUS) & STATUS_FEATURES_OK != 0,
            "virtio features rejected"
        );

        // Queue 0 setup.
        mmio_w16(common + QUEUE_SELECT, 0);
        let max = mmio_r16(common + QUEUE_SIZE);
        let qsize = max.min(64);
        mmio_w16(common + QUEUE_SIZE, qsize);
        let n = qsize as usize;

        // One page-aligned block: descriptors, avail, used (page-separated
        // for alignment simplicity), then the data buffers.
        let desc_bytes = n * 16;
        let avail_bytes = 6 + 2 * n;
        let used_bytes = 6 + 8 * n;
        let layout = Layout::from_size_align(
            (desc_bytes + 4095 & !4095)
                + (avail_bytes + 4095 & !4095)
                + (used_bytes + 4095 & !4095)
                + n * buf_len,
            4096,
        )
        .unwrap();
        let mem = unsafe { alloc_zeroed(layout) } as usize;
        let desc = mem;
        let avail = mem + (desc_bytes + 4095 & !4095);
        let used = avail + (avail_bytes + 4095 & !4095);
        let buffers = used + (used_bytes + 4095 & !4095);

        // Identity mapping: virtual address == physical address.
        mmio_w64(common + QUEUE_DESC, desc as u64);
        mmio_w64(common + QUEUE_DRIVER, avail as u64);
        mmio_w64(common + QUEUE_DEVICE, used as u64);
        mmio_w16(common + QUEUE_ENABLE, 1);

        let notify_off = mmio_r16(common + QUEUE_NOTIFY_OFF) as usize;
        let notify_addr = notify_base + notify_off * nmult;

        mmio_w8(
            common + DEVICE_STATUS,
            STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );

        let mut dev = Self {
            common,
            notify_addr,
            isr_addr,
            queue_size: qsize,
            desc,
            avail,
            used,
            buffers,
            buf_len,
            last_used: 0,
            avail_idx: 0,
        };

        // Fill descriptors and expose them all.
        for i in 0..qsize {
            dev.write_desc(i);
            dev.push_avail(i);
        }
        dev.notify();
        Some(dev)
    }

    fn write_desc(&mut self, i: u16) {
        let d = self.desc + i as usize * 16;
        mmio_w64(d, (self.buffers + i as usize * self.buf_len) as u64);
        mmio_w32(d + 8, self.buf_len as u32);
        mmio_w16(d + 12, DESC_WRITE);
        mmio_w16(d + 14, 0);
    }

    fn push_avail(&mut self, desc_id: u16) {
        let n = self.queue_size as usize;
        let slot = self.avail + 4 + (self.avail_idx as usize % n) * 2;
        mmio_w16(slot, desc_id);
        fence();
        self.avail_idx = self.avail_idx.wrapping_add(1);
        mmio_w16(self.avail + 2, self.avail_idx);
    }

    fn notify(&self) {
        fence();
        mmio_w16(self.notify_addr, 0);
    }

    /// Poll for a completed buffer; copies it out and immediately recycles it.
    pub fn poll(&mut self, out: &mut [u8]) -> bool {
        fence();
        let used_idx = mmio_r16(self.used + 2);
        if used_idx == self.last_used {
            return false;
        }
        fence();
        let n = self.queue_size as usize;
        let slot = self.used + 4 + (self.last_used as usize % n) * 8;
        let id = mmio_r32(slot) as u16;
        let len = mmio_r32(slot + 4) as usize;
        let src = self.buffers + id as usize * self.buf_len;
        let copy = out.len().min(len).min(self.buf_len);
        unsafe { core::ptr::copy_nonoverlapping(src as *const u8, out.as_mut_ptr(), copy) };
        self.last_used = self.last_used.wrapping_add(1);
        self.push_avail(id);
        self.notify();
        true
    }

    #[allow(dead_code)]
    pub fn status(&self) -> u8 {
        mmio_r8(self.common + DEVICE_STATUS)
    }

    /// Physical address of the ISR status byte; reading it deasserts INTx.
    pub fn isr_addr(&self) -> usize {
        self.isr_addr
    }
}
