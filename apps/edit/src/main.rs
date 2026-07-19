//! edit <file> — windowed text editor (Phase 4 eviction of the kernel
//! editor and Notes; the launcher's Notes is `edit /notes.txt`). Ctrl+S
//! saves over the fs protocol; unsaved changes are flushed on close.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use abi::fs::FS_NOT_FOUND;
use abi::keys::KEY_S;
use tinyos_app::gfx::{self, Canvas, Rect};
use tinyos_app::textpad::{TextPad, LINE_H};
use tinyos_app::wait::uptime_us;
use tinyos_app::window::{Event, Window};
use tinyos_app::{app, fs, println, Env};

fn main(env: Env) -> i32 {
    let path = env
        .args
        .first()
        .cloned()
        .unwrap_or_else(|| String::from("/notes.txt"));
    let text = match fs::read(&path) {
        Ok(d) => String::from_utf8_lossy(&d).into_owned(),
        Err(FS_NOT_FOUND) => String::new(),
        Err(st) => {
            println!("edit: {path}: fs error {st}");
            return 1;
        }
    };
    let mut pad = TextPad::new(&text);
    let title = path.rsplit('/').next().unwrap_or(&path);
    let (w, h) = (560u32, 400u32);
    let Ok(mut win) = Window::open(env.shell, w, h, title) else {
        return 1;
    };
    let mut back = alloc::vec![0u32; (w * h) as usize];
    let mut events = alloc::vec::Vec::new();
    let mut note = String::new();

    loop {
        let blink = uptime_us() / 500_000 % 2 == 0;
        {
            let mut c = Canvas::new(&mut back, w as i32, h as i32);
            c.clear(gfx::BG);
            let status_h = LINE_H + 4;
            pad.render(
                &mut c,
                Rect::new(8, 6, w as i32 - 16, h as i32 - status_h - 10),
                blink,
            );
            let dirty = if pad.dirty { " [+]" } else { "" };
            c.fill_rect(Rect::new(0, h as i32 - status_h, w as i32, status_h), gfx::CARD);
            c.draw_ui_text(
                8,
                h as i32 - status_h + 4,
                &format!("{path}{dirty}   ^S save   {note}"),
                gfx::TX2,
            );
        }
        win.present_from(&back);

        events.clear();
        win.poll_events(&mut events);
        for ev in events.drain(..) {
            match ev {
                Event::Char(ch) => pad.on_char(ch),
                Event::Key { code, down: true } => pad.on_key(code),
                Event::Ctrl(KEY_S) => {
                    match fs::write(&path, pad.text().as_bytes()) {
                        Ok(()) => {
                            pad.dirty = false;
                            note = String::from("saved");
                        }
                        Err(st) => note = format!("save failed ({st})"),
                    }
                }
                Event::CloseRequested => {
                    // Flush unsaved changes on the way out (Notes semantics).
                    if pad.dirty {
                        let _ = fs::write(&path, pad.text().as_bytes());
                    }
                    return 0;
                }
                _ => {}
            }
        }
        // Caret blink cadence while idle.
        win.wait(uptime_us() + 500_000);
    }
}

app!(main);
