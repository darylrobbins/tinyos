#![no_std]
#![no_main]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

extern crate alloc;

#[macro_use]
mod logger;
mod apps;
mod arch;
mod drivers;
mod fs;
mod gfx;
mod mem;
mod obj;
mod sched;
mod smoke;
mod svc;
mod term;
mod ui;

pub const VERSION: &str = "0.1.0";

static FB_SIZE: spin::Once<(usize, usize)> = spin::Once::new();

pub fn fb_size() -> (usize, usize) {
    *FB_SIZE.get().unwrap_or(&(0, 0))
}

use uefi::boot::{self, MemoryType};
use uefi::mem::memory_map::MemoryMapOwned;
use uefi::prelude::*;
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};

use gfx::{FbFormat, FbInfo};

#[entry]
fn main() -> Status {
    kprintln!("tinyos: booting at {}", arch::boot_privilege());

    let fb = setup_graphics().expect("graphics init failed");
    kprintln!(
        "tinyos: framebuffer {}x{} stride={} format={:?}",
        fb.width,
        fb.height,
        fb.stride,
        fb.format
    );

    #[cfg(target_arch = "x86_64")]
    arch::smp::park_aps();

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
fn kmain(mut fb: FbInfo, memory_map: MemoryMapOwned) -> ! {
    arch::exceptions::install();

    let heap_bytes = mem::init_heap(&memory_map);
    let (pool_bytes, _) = mem::frames::pool_stats();
    kprintln!(
        "tinyos: heap {} MiB, frame pool {} MiB",
        heap_bytes / (1024 * 1024),
        pool_bytes / (1024 * 1024)
    );

    #[cfg(target_arch = "aarch64")]
    {
        arch::paging::init_cpu();
        kprintln!(
            "tinyos: user paging (ttbr1) {}",
            if arch::paging::self_test() { "ok" } else { "FAILED" }
        );
    }

    // Upgrade the display: re-point ramfb at our own, larger framebuffer.
    // Gated serial mirror of shell/app output for the headless smoke harness.
    smoke::init();

    // (edk2's GOP tops out at 1024x768; ramfb itself has no such limit.)
    let (rw, rh) = drivers::fwcfg::read_str("opt/tinyos/res")
        .and_then(|s| {
            let (w, h) = s.trim().split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1440usize, 900usize));
    if (rw, rh) != (fb.width, fb.height) {
        match drivers::fwcfg::ramfb_resize(rw, rh) {
            Some(base) => {
                fb = FbInfo {
                    base,
                    width: rw,
                    height: rh,
                    stride: rw,
                    format: FbFormat::Bgrx,
                };
                kprintln!("tinyos: ramfb resized to {rw}x{rh}");
            }
            None => kprintln!("tinyos: ramfb resize unavailable, keeping GOP mode"),
        }
    }

    let mut fonts = gfx::font::Fonts::load();
    let mut surface = gfx::surface::Surface::new(fb.width, fb.height);
    kprintln!("tinyos: fonts loaded, surface ready");

    ui::splash::run(&fb, &mut surface, &mut fonts);
    kprintln!("tinyos: splash done (uptime {} ms)", arch::timer::uptime_ms());

    FB_SIZE.call_once(|| (fb.width, fb.height));
    let (input, blk) = drivers::probe();
    fs::init(blk);
    // Standing FS/PROC broker servers (pumped by the ui_thread below).
    svc::init();
    arch::irq::init();
    kprintln!("tinyos: starting scheduler on cpu{}", arch::cpu_id());

    // Hand the UI's owned state to its thread; spin::Once is just transport.
    UI_STATE.call_once(|| spin::Mutex::new(Some((fb, surface, fonts, input))));
    sched::start(ui_thread_main)
}

type UiState = (
    FbInfo,
    gfx::surface::Surface,
    gfx::font::Fonts,
    drivers::input::Input,
);
static UI_STATE: spin::Once<spin::Mutex<Option<UiState>>> = spin::Once::new();

fn ui_thread_main() {
    let (fb, mut surface, mut fonts, mut input) =
        UI_STATE.get().unwrap().lock().take().expect("ui state");
    crate::arch::smp::start_secondary_cpus();

    let mut shell = ui::shell::Shell::new(fb.width, fb.height);
    kprintln!("tinyos: shell up");
    obj::usertest::boot_hook();

    let mut events = alloc::vec::Vec::new();
    let mut deadline = 0u64;
    let mut last_log_us = 0u64;
    loop {
        events.clear();
        input.poll(&mut events);
        let now = arch::timer::uptime_us();
        let frame_due = now >= deadline;
        shell.handle(&events);
        shell.pump_externals();
        crate::svc::pump();
        shell.stats_tick(events.len() as u32);
        shell.pump_app_requests();

        // Render only when something can have changed.
        if !events.is_empty() || frame_due {
            shell.compose(&mut surface, &mut fonts);
            surface.present(&fb);
        }

        // Periodic serial heartbeat with wake statistics.
        if now.saturating_sub(last_log_us) >= 5_000_000 {
            let (wakes, idle) = arch::irq::wake_stats(0);
            kprintln!("tinyos: wakes/s={wakes} idle={idle}%");
            last_log_us = now;
        }

        deadline = shell.next_deadline(now);
        sched::waitq::INPUT.block_current(deadline);
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe { logger::force_unlock() };
    kprintln!("\n*** KERNEL PANIC ***\n{info}");
    arch::park()
}
