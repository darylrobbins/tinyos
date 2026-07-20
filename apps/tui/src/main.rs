//! Full-screen text-surface demo (terminal spec M3): draws a boxed menu with
//! textui, navigates with arrow keys, follows terminal resizes, quits on q.

#![no_std]
#![no_main]

extern crate alloc;

use abi::console::{CURSOR_BLOCK, INPUT_MODE_KEYS};
use abi::keys;
use textui::{CellBuffer, Style, ATTR_BOLD, ATTR_DIM, ATTR_INVERSE, ATTR_UNDERLINE};
use tinyos_app::{app, entry, ConsoleEvent, Env};

const ITEMS: &[&str] = &[
    "Cell surfaces are zero-copy shared memory",
    "Damage rects keep presents minimal",
    "Wide glyphs work: 漢字 ｶﾅ",
    "Underline and dim and inverse render",
    "Resize the window - the app follows",
];

const ACC: u32 = abi::tokens::ACC;
const TX2: u32 = abi::tokens::TX2;

fn render(buf: &mut CellBuffer, sel: usize) {
    buf.clear(Style::default());
    let (cols, rows) = (buf.cols, buf.rows);
    buf.draw_box(0, 0, cols, rows, Style::fg(TX2));
    buf.put_str(2, 0, " tui demo ", Style::new(ACC, 0, ATTR_BOLD));
    for (i, item) in ITEMS.iter().enumerate() {
        let y = 2 + i;
        if y + 1 >= rows {
            break;
        }
        let style = if i == sel {
            Style::new(ACC, 0, ATTR_INVERSE)
        } else {
            Style::fg(0)
        };
        buf.put_str(3, y, item, style);
    }
    if rows > 1 {
        buf.put_str(
            2,
            rows - 1,
            " up/down: select   q: quit ",
            Style::new(TX2, 0, ATTR_UNDERLINE | ATTR_DIM),
        );
    }
}

fn main(_env: Env) -> i32 {
    let con = entry::console().expect("no console");
    con.set_input_mode(INPUT_MODE_KEYS);

    // The terminal sends RESIZE right after start; that's our geometry.
    let (mut cols, mut rows) = loop {
        match con.next_event() {
            Some(ConsoleEvent::Resize { cols, rows }) => break (cols, rows),
            Some(_) => continue,
            None => return 1,
        }
    };

    let mut surf = match con.open_surface(cols, rows) {
        Ok(s) => s,
        Err(_) => return 1,
    };
    let mut buf = CellBuffer::new(cols as usize, rows as usize);
    let mut prev: Option<CellBuffer> = None;
    let mut sel = 0usize;

    loop {
        render(&mut buf, sel);
        // Diff against the last frame; present only the damaged rect.
        let damage = match &prev {
            Some(p) => buf.diff(p),
            None => Some((0, 0, buf.cols, buf.rows)),
        };
        if let Some((x, y, w, h)) = damage {
            surf.cells().copy_from_slice(buf.cells());
            surf.present(x as u32, y as u32, w as u32, h as u32);
        }
        surf.set_cursor(2 + sel as u32, 1, CURSOR_BLOCK, true);
        prev = Some(buf.clone());

        match con.next_event() {
            Some(ConsoleEvent::Char('q')) | Some(ConsoleEvent::CloseRequested) | None => break,
            Some(ConsoleEvent::Key { code: keys::UP, down: true, .. }) => {
                sel = sel.saturating_sub(1);
            }
            Some(ConsoleEvent::Key { code: keys::DOWN, down: true, .. }) => {
                sel = (sel + 1).min(ITEMS.len() - 1);
            }
            Some(ConsoleEvent::Resize { cols: c, rows: r }) if (c, r) != (cols, rows) => {
                // Open-replaces-open: new memobj at the new geometry.
                (cols, rows) = (c, r);
                surf = match con.open_surface(cols, rows) {
                    Ok(s) => s,
                    Err(_) => break,
                };
                buf.resize(cols as usize, rows as usize);
                prev = None;
            }
            Some(_) => {}
        }
    }
    surf.close();
    0
}

app!(main);

// Draws to a console text surface (open_surface) — console only.
tinyos_app::declare_caps!(b"console");
