//! clock [secs] — the in-kernel Clock/Timer app rebuilt as an SDK window
//! app (Phase 4 parity port). No args: a clock. With secs: a countdown.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec::Vec;

use tinyos_app::gfx::{self, Canvas};
use tinyos_app::wait::uptime_us;
use tinyos_app::window::{Event, Window};
use tinyos_app::{app, Env};

fn main(env: Env) -> i32 {
    let timer_secs: Option<u64> = env.args.first().and_then(|a| a.parse().ok());
    let title = if timer_secs.is_some() { "Timer" } else { "Clock" };
    let (w, h) = (300u32, 120u32);
    let Ok(mut win) = Window::open(env.shell, w, h, title) else {
        return 1;
    };
    let start_ms = uptime_us() / 1000;
    let end_ms = timer_secs.map(|s| start_ms + s * 1000);
    let mut back = alloc::vec![0u32; (w * h) as usize];
    let mut events = Vec::new();

    loop {
        let now = uptime_us() / 1000;
        {
            let mut c = Canvas::new(&mut back, w as i32, h as i32);
            c.clear(gfx::BG);
            match end_ms {
                None => {
                    let mins = 9 * 60 + 41 + now / 60_000;
                    let text = format!("{}:{:02}", mins / 60 % 24, mins % 60);
                    c.draw_text(8, 10, &text, 5, gfx::TX);
                    c.draw_ui_text(10, 66, "Fri Jul 17 2026", gfx::TX2);
                }
                Some(end) => {
                    let remaining = end.saturating_sub(now);
                    if remaining == 0 {
                        if now / 750 % 2 == 0 {
                            c.draw_text(8, 10, "DONE", 5, gfx::ACC);
                        }
                        c.draw_ui_text(10, 66, "Timer elapsed", gfx::TX2);
                    } else {
                        let secs = remaining.div_ceil(1000);
                        let text = format!("{}:{:02}", secs / 60, secs % 60);
                        c.draw_text(8, 10, &text, 5, gfx::ACC);
                        let total = timer_secs.unwrap_or(1) * 1000;
                        let bar_w = w as i32 - 16;
                        let left = (bar_w as u64 * remaining / total.max(1)) as i32;
                        c.fill_rect(gfx::Rect::new(8, 74, bar_w, 2), gfx::CARD2);
                        c.fill_rect(gfx::Rect::new(8, 74, left, 2), gfx::ACC);
                    }
                }
            }
        }
        win.present_from(&back);

        events.clear();
        win.poll_events(&mut events);
        if events.iter().any(|e| matches!(e, Event::CloseRequested)) {
            return 0;
        }
        // Tick on the second (or fast blink when a timer has elapsed).
        let tick_us = match end_ms {
            Some(end) if now >= end => 375_000,
            _ => 1_000_000,
        };
        win.wait(uptime_us() + tick_us);
    }
}

app!(main);
