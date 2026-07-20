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
use tinyos_app::memobj;
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

fn render(win: &mut Window, term: &mut Term, surf: Option<(u64, usize, usize)>) {
    let (w, h) = (win.width as i32, win.height as i32);
    let mut back: Vec<u32> = vec![0; (win.width * win.height) as usize];
    let mut cv = Canvas::new(&mut back, w, h);
    cv.clear(BG);

    if let (Some(s), Some((va, cols, rows))) = (term.surface(), surf) {
        render_surface(&mut cv, va, cols, rows, s.cursor);
        win.present_from(&back);
        return;
    }

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

fn render_surface(cv: &mut Canvas, va: u64, cols: usize, rows: usize, cursor: (usize, usize, u32, bool)) {
    let cells = unsafe {
        core::slice::from_raw_parts(va as *const abi::console::Cell, cols * rows)
    };
    let mut buf = [0u8; 4];
    for row in 0..rows {
        for col in 0..cols {
            let Some(r) = termcore::resolve_cell(&cells[row * cols + col], TX, BG) else {
                continue; // WIDE_CONT
            };
            let x = (col * CELL_W as usize) as i32;
            let y = (row * CELL_H as usize) as i32;
            let w = CELL_W * if r.wide { 2 } else { 1 };
            if let Some(bg) = r.bg {
                cv.fill_rect(Rect::new(x, y, w, CELL_H), bg);
            }
            if let Some(g) = r.glyph {
                cv.draw_mono_text(x, y, g.encode_utf8(&mut buf), r.fg);
            }
            if r.underline {
                cv.fill_rect(Rect::new(x, y + CELL_H - 3, w, 1), r.fg);
            }
        }
    }
    // Cursor (matches kernel draw_cells shapes; no blink for simplicity — the
    // kernel blinks via caret_on, optional here).
    let (crow, ccol, shape, visible) = cursor;
    if visible && crow < rows && ccol < cols {
        let x = (ccol * CELL_W as usize) as i32;
        let y = (crow * CELL_H as usize) as i32;
        let acc = (ACC & 0x00FF_FFFF) | 0x8000_0000;
        match shape {
            abi::console::CURSOR_BAR => cv.fill_rect(Rect::new(x, y, 2, CELL_H), acc),
            abi::console::CURSOR_UNDERLINE => cv.fill_rect(Rect::new(x, y + CELL_H - 2, CELL_W, 2), acc),
            _ => cv.fill_rect(Rect::new(x, y, CELL_W, CELL_H), acc), // block
        }
    }
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
    let mut surf: Option<(u64, usize, usize)> = None; // (va, cols, rows) — the dims actually validated + mapped
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
            let op = msg.bytes.get(0..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            if op == Some(abi::console::OP_SURFACE_OPEN) && msg.bytes.len() >= 12 {
                let cols = u32::from_le_bytes(msg.bytes[4..8].try_into().unwrap()) as usize;
                let rows = u32::from_le_bytes(msg.bytes[8..12].try_into().unwrap()) as usize;
                if (1..=1000).contains(&cols) && (1..=500).contains(&rows) {
                    if let Some(&h) = msg.handles.first() {
                        let need = (cols * rows * core::mem::size_of::<abi::console::Cell>()) as u64;
                        if let Ok(sz) = memobj::size(h) {
                            if need <= sz {
                                if let Some((va, _, _)) = surf.take() { memobj::unmap(va); }
                                let len = (need + 0xFFF) & !0xFFF;
                                if let Ok(va) = memobj::map(h, 0, len) { surf = Some((va, cols, rows)); }
                            }
                        }
                    }
                }
            } else if op == Some(abi::console::OP_SURFACE_CLOSE) {
                if let Some((va, _, _)) = surf.take() { memobj::unmap(va); }
            }
            term.on_console_msg(&msg.bytes);
        }
        if term.surface().is_none() {
            if let Some((va, _, _)) = surf.take() {
                memobj::unmap(va);
            }
        }
        for m in term.take_outbound() {
            let _ = con_kern.send(&m, &[]);
        }

        if term.dirty() {
            render(&mut win, &mut term, surf);
            term.clear_dirty();
        }

        let mut items = [
            WaitItem { handle: win.handle(), want: tinyos_app::syscall::SIG_READABLE, observed: 0 },
            WaitItem { handle: con_kern.0, want: tinyos_app::syscall::SIG_READABLE, observed: 0 },
        ];
        let _ = wait_many(&mut items, uptime_us() + 50_000);
    }

    let _ = proc::kill(child.thread_id);
    child.release();
    con_kern.close();
    0
}

tinyos_app::declare_caps!(b"window\nproc\nfs:self");

app!(main);
