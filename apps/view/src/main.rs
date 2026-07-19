//! view <file> — a pager: the Phase 3 proof app. Reads a real file over the
//! file protocol and pages it on a full-screen text surface.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use abi::console::INPUT_MODE_KEYS;
use abi::keys;
use textui::{CellBuffer, Style, ATTR_DIM, ATTR_INVERSE};
use tinyos_app::{app, entry, fs, println, ConsoleEvent, Env};

const TX2: u32 = abi::tokens::TX2;

fn render(buf: &mut CellBuffer, lines: &[String], top: usize, path: &str) {
    buf.clear(Style::default());
    let rows = buf.rows.saturating_sub(1);
    for (i, line) in lines.iter().skip(top).take(rows).enumerate() {
        buf.put_str(0, i, line, Style::fg(0));
    }
    let pct = if lines.len() <= rows {
        100
    } else {
        ((top + rows) * 100 / lines.len()).min(100)
    };
    buf.put_str(
        0,
        buf.rows - 1,
        &format!(" {path}  {pct}%  (up/down/space: scroll, q: quit) "),
        Style::new(TX2, 0, ATTR_INVERSE | ATTR_DIM),
    );
}

fn main(env: Env) -> i32 {
    let Some(path) = env.args.first() else {
        println!("usage: view <file>");
        return 1;
    };
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(st) => {
            println!("view: {path}: fs error {st}");
            return 1;
        }
    };
    let text = String::from_utf8_lossy(&data);
    let lines: Vec<String> = text.lines().map(String::from).collect();

    let con = entry::console().expect("no console");
    con.set_input_mode(INPUT_MODE_KEYS);
    let (cols, rows) = loop {
        match con.next_event() {
            Some(ConsoleEvent::Resize { cols, rows }) => break (cols, rows),
            Some(_) => continue,
            None => return 1,
        }
    };
    let Ok(mut surf) = con.open_surface(cols, rows) else { return 1 };
    let mut buf = CellBuffer::new(cols as usize, rows as usize);
    let mut top = 0usize;
    let page = rows.saturating_sub(2) as usize;
    let max_top = lines.len().saturating_sub(1);

    loop {
        render(&mut buf, &lines, top, path);
        surf.cells().copy_from_slice(buf.cells());
        surf.present_all();
        match con.next_event() {
            Some(ConsoleEvent::Char('q')) | Some(ConsoleEvent::CloseRequested) | None => break,
            Some(ConsoleEvent::Char(' ')) => top = (top + page).min(max_top),
            Some(ConsoleEvent::Key { code: keys::UP, down: true, .. }) => {
                top = top.saturating_sub(1);
            }
            Some(ConsoleEvent::Key { code: keys::DOWN, down: true, .. }) => {
                top = (top + 1).min(max_top);
            }
            Some(ConsoleEvent::Resize { cols: c, rows: r }) if (c, r) != (cols, rows) => {
                // v1: fixed-size surface; re-run view after resizing.
            }
            Some(_) => {}
        }
    }
    surf.close();
    0
}

app!(main);
