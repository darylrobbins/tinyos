//! virtio-blk: synchronous, single-outstanding-request block driver.
//! Each request is a 3-descriptor chain (header / data / status) submitted
//! and then spin-polled to completion — filesystem IO is µs-scale under
//! QEMU and the FS layer is fully synchronous anyway.

use super::pci::{BarAllocator, PciDevice};
use super::virtio::{Queue, Transport, DESC_NEXT, DESC_WRITE};

pub const VIRTIO_ID_BLK: u16 = 0x1042; // modern virtio-blk PCI device id

pub const SECTOR_SIZE: usize = 512;
/// Data area per request; matches the tinyfs block size.
pub const IO_SIZE: usize = 4096;

const FEATURE_FLUSH: u32 = 1 << 9; // VIRTIO_BLK_F_FLUSH

const REQ_IN: u32 = 0; // device -> memory (read)
const REQ_OUT: u32 = 1; // memory -> device (write)
const REQ_FLUSH: u32 = 4;

const STATUS_PENDING: u8 = 0xff; // ours, not the device's

// Scratch layout (after the rings): data buffer, then header, then status.
const HDR_OFF: usize = IO_SIZE;
const STATUS_OFF: usize = IO_SIZE + 16;

pub struct VirtioBlk {
    queue: Queue,
    capacity_sectors: u64,
    has_flush: bool,
}

/// Spin briefly (fast virtio completions are µs-scale), then block on the
/// INPUT wait queue — the blk INTx sets WAKE_INPUT like the input devices,
/// so completion wakes us. Pre-scheduler (mount at boot) it just spins.
fn wait_used(queue: &Queue) {
    let t0 = crate::arch::timer::uptime_us();
    while queue.peek_used().is_none() {
        if crate::sched::started() && crate::arch::timer::uptime_us() - t0 > 200 {
            let dl = crate::arch::timer::uptime_us() + 1_000;
            crate::sched::waitq::INPUT.block_current(dl);
        } else {
            core::hint::spin_loop();
        }
    }
}

impl VirtioBlk {
    pub fn init(pci: &PciDevice, alloc: &mut BarAllocator) -> Option<Self> {
        let transport = Transport::init(pci, alloc, FEATURE_FLUSH)?;
        // Completion IRQs ride the shared PCI INTx path: register the ISR
        // byte (read = deassert) in a free slot; the handler sets
        // WAKE_INPUT, which wakes any blocked requester below.
        let _idx = crate::arch::irq::claim_isr_slot(transport.isr_addr());
        #[cfg(target_arch = "x86_64")]
        if let Some(idx) = _idx {
            crate::arch::irq::register_input_gsi(idx, pci.interrupt_line() as u32);
        }
        let queue = transport.setup_queue(0, 8, IO_SIZE + 16 + 1);
        transport.driver_ok();

        // Device config starts with capacity in 512-byte sectors (u64 LE).
        let cfg = transport.device_cfg();
        if cfg == 0 {
            return None;
        }
        let capacity_sectors = super::mmio::r32(cfg) as u64 | (super::mmio::r32(cfg + 4) as u64) << 32;

        Some(Self {
            queue,
            capacity_sectors,
            has_flush: transport.features0 & FEATURE_FLUSH != 0,
        })
    }

    pub fn capacity_sectors(&self) -> u64 {
        self.capacity_sectors
    }

    /// Submit one request over the DMA scratch area (data already staged
    /// there for writes) and spin until the device completes it.
    /// Returns false on a device-reported error.
    fn request(&mut self, req_type: u32, sector: u64) -> bool {
        let base = self.queue.extra;
        // Header: type u32, reserved u32, sector u64.
        unsafe {
            core::ptr::write_volatile((base + HDR_OFF) as *mut u32, req_type);
            core::ptr::write_volatile((base + HDR_OFF + 4) as *mut u32, 0);
            core::ptr::write_volatile((base + HDR_OFF + 8) as *mut u64, sector);
            core::ptr::write_volatile((base + STATUS_OFF) as *mut u8, STATUS_PENDING);
        }

        self.queue
            .set_desc(0, (base + HDR_OFF) as u64, 16, DESC_NEXT, 1);
        match req_type {
            REQ_FLUSH => self
                .queue
                .set_desc(1, (base + STATUS_OFF) as u64, 1, DESC_WRITE, 0),
            _ => {
                let flags = DESC_NEXT | if req_type == REQ_IN { DESC_WRITE } else { 0 };
                self.queue.set_desc(1, base as u64, IO_SIZE as u32, flags, 2);
                self.queue
                    .set_desc(2, (base + STATUS_OFF) as u64, 1, DESC_WRITE, 0);
            }
        }
        self.queue.push_avail(0);
        self.queue.notify();

        wait_used(&self.queue);
        let _ = self.queue.poll_used();
        unsafe { core::ptr::read_volatile((base + STATUS_OFF) as *const u8) == 0 }
    }

    /// Read `IO_SIZE` bytes starting at `sector`.
    pub fn read(&mut self, sector: u64, buf: &mut [u8]) -> bool {
        assert_eq!(buf.len(), IO_SIZE);
        if !self.request(REQ_IN, sector) {
            return false;
        }
        let base = self.queue.extra;
        unsafe { core::ptr::copy_nonoverlapping(base as *const u8, buf.as_mut_ptr(), IO_SIZE) };
        true
    }

    /// Write `IO_SIZE` bytes starting at `sector`.
    pub fn write(&mut self, sector: u64, buf: &[u8]) -> bool {
        assert_eq!(buf.len(), IO_SIZE);
        let base = self.queue.extra;
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), base as *mut u8, IO_SIZE) };
        self.request(REQ_OUT, sector)
    }

    /// Flush the device's write cache (no-op if the device has none).
    pub fn flush(&mut self) -> bool {
        if !self.has_flush {
            return true;
        }
        self.request(REQ_FLUSH, 0)
    }
}
