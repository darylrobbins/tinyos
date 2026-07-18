#![no_std]
#![no_main]

extern crate alloc;

#[macro_use]
mod logger;
mod arch;
mod gfx;
mod mem;

use uefi::boot::{self, MemoryType};
use uefi::mem::memory_map::MemoryMapOwned;
use uefi::prelude::*;
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};

use gfx::{FbFormat, FbInfo};

#[entry]
fn main() -> Status {
    kprintln!("tinyos: booting at EL{}", arch::current_el());

    let fb = setup_graphics().expect("graphics init failed");
    kprintln!(
        "tinyos: framebuffer {}x{} stride={} format={:?}",
        fb.width,
        fb.height,
        fb.stride,
        fb.format
    );

    kprintln!("tinyos: exiting boot services");
    let memory_map = unsafe { boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };

    kmain(fb, memory_map)
}

fn setup_graphics() -> uefi::Result<FbInfo> {
    let handle = boot::get_handle_for_protocol::<GraphicsOutput>()?;
    let mut gop = boot::open_protocol_exclusive::<GraphicsOutput>(handle)?;

    // Pick the largest available mode up to 1280x800 (ramfb offers a few).
    let mode = gop
        .modes()
        .inspect(|m| {
            let (w, h) = m.info().resolution();
            kprintln!("tinyos: gop mode {}x{} {:?}", w, h, m.info().pixel_format());
        })
        .filter(|m| {
            let (w, h) = m.info().resolution();
            w <= 1280 && h <= 800
        })
        .max_by_key(|m| {
            let (w, h) = m.info().resolution();
            w * h
        });
    if let Some(mode) = mode {
        gop.set_mode(&mode)?;
    }

    let info = gop.current_mode_info();
    let (width, height) = info.resolution();
    let format = match info.pixel_format() {
        PixelFormat::Rgb => FbFormat::Rgbx,
        PixelFormat::Bgr => FbFormat::Bgrx,
        other => panic!("unsupported pixel format {other:?}"),
    };
    let mut raw_fb = gop.frame_buffer();
    Ok(FbInfo {
        base: raw_fb.as_mut_ptr(),
        width,
        height,
        stride: info.stride(),
        format,
    })
}

/// Post-boot-services entry point. UEFI's identity-mapped page tables and
/// stack remain in use; the memory map tells us what RAM is ours to manage.
fn kmain(fb: FbInfo, memory_map: MemoryMapOwned) -> ! {
    arch::exceptions::install();

    let heap_bytes = mem::init_heap(&memory_map);
    kprintln!("tinyos: heap {} MiB", heap_bytes / (1024 * 1024));

    let v: alloc::vec::Vec<u32> = (0..64).collect();
    kprintln!("tinyos: alloc ok ({} elems)", v.len());

    gfx::test_pattern(&fb);
    kprintln!("tinyos: test pattern drawn - M1 complete");

    arch::park()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe { logger::force_unlock() };
    kprintln!("\n*** KERNEL PANIC ***\n{info}");
    arch::park()
}
