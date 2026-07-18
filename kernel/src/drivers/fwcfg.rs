//! QEMU fw_cfg (MMIO flavor, as on the virt board) plus ramfb resize.
//!
//! ramfb has no registers: its framebuffer address/size live in the
//! "etc/ramfb" fw_cfg file. Rewriting that file (DMA interface) points the
//! display at a kernel-owned buffer of any resolution — bigger than the
//! three modes edk2's GOP driver offers.

use alloc::alloc::{alloc_zeroed, Layout};
use alloc::string::String;
use alloc::vec;

use super::mmio;

/// fw_cfg MMIO on QEMU virt: data @+0, selector @+8, DMA control @+16.
const BASE: usize = 0x0902_0000;
const FILE_DIR: u16 = 0x19;

const DMA_ERROR: u32 = 1;
const DMA_SELECT: u32 = 8;
const DMA_WRITE: u32 = 16;

fn select(sel: u16) {
    mmio::w16(BASE + 8, sel.swap_bytes());
}

fn read_bytes(buf: &mut [u8]) {
    for b in buf {
        *b = mmio::r8(BASE);
    }
}

/// (selector, size) of a fw_cfg file, if present.
fn find_file(name: &str) -> Option<(u16, usize)> {
    select(FILE_DIR);
    let mut count_b = [0u8; 4];
    read_bytes(&mut count_b);
    let count = u32::from_be_bytes(count_b);
    for _ in 0..count.min(256) {
        let mut entry = [0u8; 64];
        read_bytes(&mut entry);
        let size = u32::from_be_bytes(entry[0..4].try_into().unwrap()) as usize;
        let sel = u16::from_be_bytes(entry[4..6].try_into().unwrap());
        let end = entry[8..].iter().position(|&c| c == 0).unwrap_or(56) + 8;
        if &entry[8..end] == name.as_bytes() {
            return Some((sel, size));
        }
    }
    None
}

/// Read a whole fw_cfg file as a string (for -fw_cfg name=...,string=...).
pub fn read_str(name: &str) -> Option<String> {
    let (sel, size) = find_file(name)?;
    select(sel);
    let mut buf = vec![0u8; size.min(256)];
    read_bytes(&mut buf);
    String::from_utf8(buf).ok()
}

/// DMA-write `data` into the fw_cfg file at `sel`. Returns false on error.
fn dma_write(sel: u16, data: &[u8]) -> bool {
    // FWCfgDmaAccess: be32 control, be32 length, be64 address.
    let mut access = [0u8; 16];
    let control = (sel as u32) << 16 | DMA_SELECT | DMA_WRITE;
    access[0..4].copy_from_slice(&control.to_be_bytes());
    access[4..8].copy_from_slice(&(data.len() as u32).to_be_bytes());
    access[8..16].copy_from_slice(&(data.as_ptr() as u64).to_be_bytes());

    let access_addr = access.as_ptr() as u64;
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    mmio::w64(BASE + 16, access_addr.swap_bytes());
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    let ctl = u32::from_be_bytes(access[0..4].try_into().unwrap());
    ctl & DMA_ERROR == 0
}

/// Point ramfb at a freshly allocated framebuffer of the given size.
/// Returns the buffer address, or None if ramfb is absent or the write fails.
pub fn ramfb_resize(width: usize, height: usize) -> Option<*mut u8> {
    let (sel, _) = find_file("etc/ramfb")?;

    let stride = width * 4;
    let layout = Layout::from_size_align(stride * height, 4096).ok()?;
    let fb = unsafe { alloc_zeroed(layout) };
    if fb.is_null() {
        return None;
    }

    // RAMFBCfg, all fields big-endian: addr, fourcc, flags, width, height, stride.
    const XRGB8888: u32 = 0x3432_5258; // DRM fourcc 'XR24'
    let mut cfg = [0u8; 28];
    cfg[0..8].copy_from_slice(&(fb as u64).to_be_bytes());
    cfg[8..12].copy_from_slice(&XRGB8888.to_be_bytes());
    cfg[16..20].copy_from_slice(&(width as u32).to_be_bytes());
    cfg[20..24].copy_from_slice(&(height as u32).to_be_bytes());
    cfg[24..28].copy_from_slice(&(stride as u32).to_be_bytes());

    dma_write(sel, &cfg).then_some(fb)
}
