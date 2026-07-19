//! Modern (non-legacy) virtio-pci transport with polled split virtqueues.
//! No interrupts for data flow: used rings are polled.
//!
//! `Transport` + `Queue` hold the device-generic pieces (capability walk,
//! reset/feature negotiation, ring setup); `VirtioDevice` keeps the
//! virtio-input shape (one queue of pre-posted device-writable buffers) and
//! `virtio_blk::VirtioBlk` builds request/response chains on the same
//! primitives.

use alloc::alloc::{alloc_zeroed, Layout};

use super::mmio::{r8 as mmio_r8, r16 as mmio_r16, r32 as mmio_r32, w8 as mmio_w8, w16 as mmio_w16, w32 as mmio_w32, w64 as mmio_w64};
use super::pci::{BarAllocator, PciDevice};

const CAP_VENDOR: u8 = 0x09;
const CFG_COMMON: u8 = 1;
const CFG_NOTIFY: u8 = 2;
const CFG_ISR: u8 = 3;
const CFG_DEVICE: u8 = 4;

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

pub const DESC_NEXT: u16 = 1;
pub const DESC_WRITE: u16 = 2;

fn fence() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// Device-generic half of a modern virtio-pci device: mapped config
/// structures with reset + feature negotiation done, queues not yet set up.
pub struct Transport {
    common: usize,
    notify_base: usize,
    notify_mult: usize,
    isr_addr: usize,
    device_cfg: usize,
    /// Feature word 0 accepted by the device (VERSION_1 lives in word 1).
    pub features0: u32,
}

impl Transport {
    /// Reset the device and negotiate VERSION_1 plus whatever subset of
    /// `want_features0` (feature bits 0..31) the device offers.
    pub fn init(pci: &PciDevice, alloc: &mut BarAllocator, want_features0: u32) -> Option<Self> {
        pci.enable();

        // Walk capabilities for the virtio config structures.
        let mut common = None;
        let mut notify = None; // (bar, offset, multiplier)
        let mut isr = None;
        let mut device = None;
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
                    CFG_DEVICE => device = Some((bar, offset)),
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
        let device_cfg = match device {
            Some((dbar, doff)) => pci.bar_addr(dbar, alloc)? as usize + doff,
            None => 0,
        };

        // Reset, then acknowledge.
        mmio_w8(common + DEVICE_STATUS, 0);
        while mmio_r8(common + DEVICE_STATUS) != 0 {}
        mmio_w8(common + DEVICE_STATUS, STATUS_ACK);
        mmio_w8(common + DEVICE_STATUS, STATUS_ACK | STATUS_DRIVER);

        // Features: VERSION_1 required, word 0 negotiated down to the offer.
        mmio_w32(common + DEVICE_FEATURE_SELECT, 1);
        let f1 = mmio_r32(common + DEVICE_FEATURE);
        assert!(f1 & FEATURE_VERSION_1 != 0, "legacy-only virtio device");
        mmio_w32(common + DEVICE_FEATURE_SELECT, 0);
        let offered0 = mmio_r32(common + DEVICE_FEATURE);
        let features0 = offered0 & want_features0;
        mmio_w32(common + DRIVER_FEATURE_SELECT, 0);
        mmio_w32(common + DRIVER_FEATURE, features0);
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

        Some(Self {
            common,
            notify_base,
            notify_mult: nmult,
            isr_addr,
            device_cfg,
            features0,
        })
    }

    /// Set up split virtqueue `index` with at most `max_size` descriptors,
    /// plus `extra_bytes` of DMA-reachable scratch after the rings.
    pub fn setup_queue(&self, index: u16, max_size: u16, extra_bytes: usize) -> Queue {
        mmio_w16(self.common + QUEUE_SELECT, index);
        let max = mmio_r16(self.common + QUEUE_SIZE);
        let qsize = max.min(max_size);
        mmio_w16(self.common + QUEUE_SIZE, qsize);
        let n = qsize as usize;

        // One page-aligned allocation: descriptors, avail, used
        // (page-separated for alignment simplicity), then scratch.
        let desc_bytes = n * 16;
        let avail_bytes = 6 + 2 * n;
        let used_bytes = 6 + 8 * n;
        let layout = Layout::from_size_align(
            (desc_bytes + 4095 & !4095)
                + (avail_bytes + 4095 & !4095)
                + (used_bytes + 4095 & !4095)
                + extra_bytes,
            4096,
        )
        .unwrap();
        let mem = unsafe { alloc_zeroed(layout) } as usize;
        let desc = mem;
        let avail = mem + (desc_bytes + 4095 & !4095);
        let used = avail + (avail_bytes + 4095 & !4095);
        let extra = used + (used_bytes + 4095 & !4095);

        // Identity mapping: virtual address == physical address.
        mmio_w64(self.common + QUEUE_DESC, desc as u64);
        mmio_w64(self.common + QUEUE_DRIVER, avail as u64);
        mmio_w64(self.common + QUEUE_DEVICE, used as u64);
        mmio_w16(self.common + QUEUE_ENABLE, 1);

        let notify_off = mmio_r16(self.common + QUEUE_NOTIFY_OFF) as usize;
        let notify_addr = self.notify_base + notify_off * self.notify_mult;

        Queue {
            size: qsize,
            desc,
            avail,
            used,
            extra,
            notify_addr,
            last_used: 0,
            avail_idx: 0,
        }
    }

    /// All queues configured; let the device run.
    pub fn driver_ok(&self) {
        mmio_w8(
            self.common + DEVICE_STATUS,
            STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );
    }

    /// Device-specific config area (0 if the device exposes none).
    pub fn device_cfg(&self) -> usize {
        self.device_cfg
    }

    /// Physical address of the ISR status byte; reading it deasserts INTx.
    pub fn isr_addr(&self) -> usize {
        self.isr_addr
    }
}

/// One split virtqueue plus its scratch area.
pub struct Queue {
    pub size: u16,
    desc: usize,
    avail: usize,
    used: usize,
    /// DMA-reachable scratch after the rings (`extra_bytes` at setup).
    pub extra: usize,
    notify_addr: usize,
    last_used: u16,
    avail_idx: u16,
}

impl Queue {
    pub fn set_desc(&self, i: u16, addr: u64, len: u32, flags: u16, next: u16) {
        let d = self.desc + i as usize * 16;
        mmio_w64(d, addr);
        mmio_w32(d + 8, len);
        mmio_w16(d + 12, flags);
        mmio_w16(d + 14, next);
    }

    pub fn push_avail(&mut self, desc_id: u16) {
        let n = self.size as usize;
        let slot = self.avail + 4 + (self.avail_idx as usize % n) * 2;
        mmio_w16(slot, desc_id);
        fence();
        self.avail_idx = self.avail_idx.wrapping_add(1);
        mmio_w16(self.avail + 2, self.avail_idx);
    }

    pub fn notify(&self) {
        fence();
        mmio_w16(self.notify_addr, 0);
    }

    /// Next completed buffer, if any: (descriptor id, written length).
    pub fn poll_used(&mut self) -> Option<(u16, u32)> {
        fence();
        let used_idx = mmio_r16(self.used + 2);
        if used_idx == self.last_used {
            return None;
        }
        fence();
        let n = self.size as usize;
        let slot = self.used + 4 + (self.last_used as usize % n) * 8;
        let id = mmio_r32(slot) as u16;
        let len = mmio_r32(slot + 4);
        self.last_used = self.last_used.wrapping_add(1);
        Some((id, len))
    }
}

/// A device with one device-writable queue of fixed-size buffers
/// (all virtio-input needs).
pub struct VirtioDevice {
    queue: Queue,
    isr_addr: usize,
    buf_len: usize,
}

impl VirtioDevice {
    /// Initialize queue 0 with device-writable buffers of `buf_len`.
    pub fn init(pci: &PciDevice, alloc: &mut BarAllocator, buf_len: usize) -> Option<Self> {
        let transport = Transport::init(pci, alloc, 0)?;
        let mut queue = transport.setup_queue(0, 64, 64 * buf_len);
        transport.driver_ok();

        // Fill descriptors and expose them all.
        for i in 0..queue.size {
            queue.set_desc(
                i,
                (queue.extra + i as usize * buf_len) as u64,
                buf_len as u32,
                DESC_WRITE,
                0,
            );
            queue.push_avail(i);
        }
        queue.notify();
        Some(Self {
            queue,
            isr_addr: transport.isr_addr(),
            buf_len,
        })
    }

    /// Poll for a completed buffer; copies it out and immediately recycles it.
    pub fn poll(&mut self, out: &mut [u8]) -> bool {
        let Some((id, len)) = self.queue.poll_used() else {
            return false;
        };
        let src = self.queue.extra + id as usize * self.buf_len;
        let copy = out.len().min(len as usize).min(self.buf_len);
        unsafe { core::ptr::copy_nonoverlapping(src as *const u8, out.as_mut_ptr(), copy) };
        self.queue.push_avail(id);
        self.queue.notify();
        true
    }

    /// Physical address of the ISR status byte; reading it deasserts INTx.
    pub fn isr_addr(&self) -> usize {
        self.isr_addr
    }
}
