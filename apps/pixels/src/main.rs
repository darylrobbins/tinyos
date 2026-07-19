#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use tinyos_app::app;
use tinyos_app::entry::Env;
use tinyos_app::wait::uptime_us;
use tinyos_app::window::{Event, Window};

fn main(env: Env) -> i32 {
    let mut win = match Window::open(env.shell, 320, 200, "pixels") {
        Ok(w) => w,
        Err(_) => return 1,
    };
    let (w, h) = (win.width, win.height);
    let mut events = Vec::new();
    loop {
        let t = (uptime_us() / 16_000) as u32; // ~60 steps/sec
        let px = win.pixels();
        for y in 0..h {
            for x in 0..w {
                let r = ((x + t) & 0xFF) as u32;
                let g = ((y + t / 2) & 0xFF) as u32;
                let b = ((x + y + t) & 0xFF) as u32;
                px[(y * w + x) as usize] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            }
        }
        win.present();

        events.clear();
        win.poll_events(&mut events);
        for e in &events {
            if let Event::CloseRequested = e {
                return 0;
            }
        }
        win.wait(uptime_us() + 33_000); // ~30 fps
    }
}

app!(main);
