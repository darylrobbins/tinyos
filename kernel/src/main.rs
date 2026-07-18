#![no_std]
#![no_main]

extern crate alloc;

#[macro_use]
mod logger;
mod arch;
mod drivers;
mod gfx;
mod mem;
mod ui;

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

    let mut fonts = gfx::font::Fonts::load();
    let mut surface = gfx::surface::Surface::new(fb.width, fb.height);
    kprintln!("tinyos: fonts loaded, surface ready");

    ui::splash::run(&fb, &mut surface, &mut fonts);
    kprintln!("tinyos: splash done (uptime {} ms)", arch::timer::uptime_ms());

    let input = drivers::input::Input::init();
    input_demo(&fb, &mut surface, &mut fonts, input)
}

/// M3 demo: echo keys, track the pointer, draw the cursor.
fn input_demo(
    fb: &FbInfo,
    surface: &mut gfx::surface::Surface,
    fonts: &mut gfx::font::Fonts,
    mut input: drivers::input::Input,
) -> ! {
    use drivers::input::{keycode_to_char, keys, Event, ABS_MAX};
    use gfx::surface::rgb;

    let mut events = alloc::vec::Vec::new();
    let (mut px, mut py) = (surface.width as i32 / 2, surface.height as i32 / 2);
    let mut shift = false;
    let mut typed = alloc::string::String::new();
    let mut buttons = (false, false);

    loop {
        events.clear();
        input.poll(&mut events);
        for ev in &events {
            match *ev {
                Event::PointerX(v) => px = (v * (surface.width as u32 - 1) / ABS_MAX) as i32,
                Event::PointerY(v) => py = (v * (surface.height as u32 - 1) / ABS_MAX) as i32,
                Event::Button { right, down } => {
                    if right {
                        buttons.1 = down
                    } else {
                        buttons.0 = down
                    }
                }
                Event::Key { code, down } => {
                    if code == keys::LSHIFT || code == keys::RSHIFT {
                        shift = down;
                    } else if down {
                        if code == keys::BACKSPACE {
                            typed.pop();
                        } else if let Some(c) = keycode_to_char(code, shift) {
                            typed.push(c);
                        }
                    }
                }
                Event::Frame => {}
            }
        }

        surface.clear(rgb(14, 14, 20));
        fonts.ui_medium.draw(surface, "tinyOS input demo", 28.0, 40, 40, rgb(240, 240, 245));
        let line = alloc::format!("typed: {typed}");
        fonts.mono.draw(surface, &line, 20.0, 40, 110, rgb(140, 220, 200));
        let stat = alloc::format!(
            "pointer: {px},{py}  buttons: L={} R={}",
            buttons.0 as u8,
            buttons.1 as u8
        );
        fonts.mono.draw(surface, &stat, 20.0, 40, 150, rgb(160, 160, 180));
        ui::cursor::draw(surface, px, py);
        surface.present(fb);

        let next = arch::timer::uptime_us() / 16_667 * 16_667 + 16_667;
        arch::timer::wait_until_us(next);
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe { logger::force_unlock() };
    kprintln!("\n*** KERNEL PANIC ***\n{info}");
    arch::park()
}
