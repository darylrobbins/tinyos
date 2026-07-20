//! terminal — the userspace terminal I/O wrapper (SP1a, line world). Opens a
//! window, spawns `/apps/sh` into a console channel it serves, drives the
//! pure `termcore::Term` model with events from both sides, and renders the
//! result with the mono atlas. Mirrors how `apps/vi` wraps `crates/vicore`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use abi::bootstrap::{TAG_CONSOLE, TAG_FS, TAG_FS_BROKER, TAG_PROC, TAG_PROC_BROKER};
use abi::keys;
use abi::tokens::{ACC, BG, TX};
use termcore::Term;
use tinyos_app::channel::Channel;
use tinyos_app::entry::Env;
use tinyos_app::gfx::{Canvas, Rect};
use tinyos_app::monofont::{ADVANCE as CELL_W, LINE_H as CELL_H};
use tinyos_app::syscall::{syscall2, RIGHTS_ALL, SYS_HANDLE_DUP};
use tinyos_app::wait::{uptime_us, wait_many, WaitItem};
use tinyos_app::window::{Event, Window};
use tinyos_app::{app, broker, fs, process, proc};

const COLS: u32 = 80;
const ROWS: u32 = 24;
const WIDTH: u32 = COLS * CELL_W as u32;
const HEIGHT: u32 = ROWS * CELL_H as u32;

/// Duplicate a handle with full rights (for grants the child should also
/// hold, e.g. the broker channels themselves alongside a fresh connection).
fn dup(handle: u32) -> u32 {
    syscall2(SYS_HANDLE_DUP, handle as u64, RIGHTS_ALL as u64).value as u32
}

fn render(win: &mut Window, term: &mut Term) {
    let (w, h) = (win.width as i32, win.height as i32);
    let mut back: Vec<u32> = vec![0; (win.width * win.height) as usize];
    let mut cv = Canvas::new(&mut back, w, h);
    cv.clear(BG);

    let rows = (h / CELL_H).max(1) as usize;
    // Bottom row is reserved for the prompt + input line.
    let text_rows = rows.saturating_sub(1);
    let lines: Vec<_> = term.scrollback().collect();
    let start = lines.len().saturating_sub(text_rows);
    for (i, line) in lines[start..].iter().enumerate() {
        cv.draw_mono_text(0, i as i32 * CELL_H, &line.text, line.color);
    }

    // Prompt spans + input, on the bottom line.
    let y = text_rows as i32 * CELL_H;
    let mut x = 0i32;
    for (text, color) in term.prompt() {
        cv.draw_mono_text(x, y, &text, color);
        x += text.chars().count() as i32 * CELL_W;
    }
    let input = term.input();
    cv.draw_mono_text(x, y, input, TX);

    // Cursor: a filled cell at the input's cursor column.
    let cursor_x = x + term.cursor() as i32 * CELL_W;
    cv.fill_rect(Rect::new(cursor_x, y, CELL_W, CELL_H), (ACC & 0x00FF_FFFF) | 0x8000_0000);

    win.present_from(&back);
}

fn main(env: Env) -> i32 {
    let mut win = match Window::open(env.shell, WIDTH, HEIGHT, "terminal") {
        Ok(w) => w,
        Err(_) => return 1,
    };
    let cols = (win.width / CELL_W as u32) as usize;
    let rows = (win.height / CELL_H as u32) as usize;

    // con_app (client) goes to sh as TAG_CONSOLE; con_kern (server) stays here.
    let (con_app, con_kern) = match Channel::create() {
        Ok(pair) => pair,
        Err(_) => return 1,
    };

    let elf = match fs::read("/apps/sh") {
        Ok(d) => d,
        Err(_) => return 1,
    };
    let fs_conn = match broker::connect(env.fs_broker) {
        Ok(c) => c,
        Err(_) => return 1,
    };
    let proc_conn = match broker::connect(env.proc_broker) {
        Ok(c) => c,
        Err(_) => return 1,
    };
    let grants = [
        (TAG_CONSOLE, con_app.0),
        (TAG_FS, fs_conn.0),
        (TAG_PROC, proc_conn.0),
        (TAG_FS_BROKER, dup(env.fs_broker.0)),
        (TAG_PROC_BROKER, dup(env.proc_broker.0)),
    ];
    let child = match process::spawn(&elf, &[], &grants) {
        Ok(c) => c,
        Err(_) => return 1,
    };

    let mut term = Term::new();
    term.set_size(cols, rows);
    for m in term.take_outbound() {
        let _ = con_kern.send(&m, &[]);
    }

    let mut events = Vec::new();
    loop {
        events.clear();
        win.poll_events(&mut events);
        let mut close = false;
        for e in &events {
            match *e {
                Event::Char(c) => term.on_char(c),
                Event::Key { code, down: true } => term.on_key(code),
                Event::Key { down: false, .. } => {}
                Event::Ctrl(code) => {
                    if code == keys::KEY_C {
                        let _ = proc::kill(term.foreground_tid());
                    }
                }
                Event::PointerMoved { .. } | Event::Button { .. } => {}
                Event::CloseRequested => close = true,
            }
        }
        if close {
            break;
        }

        while let Ok(msg) = con_kern.try_recv() {
            term.on_console_msg(&msg.bytes);
        }
        for m in term.take_outbound() {
            let _ = con_kern.send(&m, &[]);
        }

        if term.dirty() {
            render(&mut win, &mut term);
            term.clear_dirty();
        }

        let mut items = [
            WaitItem { handle: win.handle(), want: tinyos_app::syscall::SIG_READABLE, observed: 0 },
            WaitItem { handle: con_kern.0, want: tinyos_app::syscall::SIG_READABLE, observed: 0 },
        ];
        let _ = wait_many(&mut items, uptime_us() + 50_000);
    }

    child.release();
    con_kern.close();
    0
}

tinyos_app::declare_caps!(b"window\nproc\nfs:self");

app!(main);
