//! vi — the vicore engine as a userspace terminal app (Phase 4 eviction).
//! Files over the fs protocol, display over a cell surface, keys over the
//! console protocol. The engine is pure; this host performs its Effects.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use abi::console::{CURSOR_BAR, CURSOR_BLOCK, INPUT_MODE_KEYS};
use abi::fs::FS_NOT_FOUND;
use abi::keys;
use textui::{CellBuffer, Style, ATTR_INVERSE};
use tinyos_app::{app, entry, fs, println, ConsoleEvent, Env};
use vicore::editor::{Effect, Mode, Special};
use vicore::Editor;

const ACC: u32 = abi::tokens::ACC;

fn render(buf: &mut CellBuffer, ed: &Editor, path: &str) {
    buf.clear(Style::default());
    let text_rows = buf.rows.saturating_sub(1);
    let top = ed.view_top();
    for (i, line) in ed.lines().iter().skip(top).take(text_rows).enumerate() {
        buf.put_str(0, i, line, Style::fg(0));
    }
    for i in ed.lines().len().saturating_sub(top)..text_rows {
        buf.put_str(0, i, "~", Style::fg(ACC));
    }
    // Status line: `:`/`/` command echo wins, else mode + file + engine status.
    let status = match ed.command_line() {
        Some(cmd) => cmd,
        None => {
            let dirty = if ed.is_dirty() { " [+]" } else { "" };
            format!(" {}  {path}{dirty}  {}", ed.mode_label(), ed.status())
        }
    };
    buf.put_str(0, buf.rows - 1, &status, Style::new(0, 0, ATTR_INVERSE));
    for x in status.chars().count()..buf.cols {
        buf.put(x, buf.rows - 1, ' ', Style::new(0, 0, ATTR_INVERSE));
    }
}

fn main(env: Env) -> i32 {
    let Some(path) = env.args.first().cloned() else {
        println!("usage: vi <file>");
        return 1;
    };
    let text = match fs::read(&path) {
        Ok(d) => String::from_utf8_lossy(&d).into_owned(),
        Err(FS_NOT_FOUND) => String::new(),
        Err(st) => {
            println!("vi: {path}: fs error {st}");
            return 1;
        }
    };
    let mut ed = Editor::new(&text);

    let con = entry::console().expect("no console");
    con.set_input_mode(INPUT_MODE_KEYS);
    let (mut cols, mut rows) = loop {
        match con.next_event() {
            Some(ConsoleEvent::Resize { cols, rows }) => break (cols, rows),
            Some(_) => continue,
            None => return 1,
        }
    };
    let Ok(mut surf) = con.open_surface(cols, rows) else { return 1 };
    let mut buf = CellBuffer::new(cols as usize, rows as usize);

    loop {
        ed.set_view_rows(rows.saturating_sub(1) as usize);
        let top = ed.scroll_into_view();
        render(&mut buf, &ed, &path);
        surf.cells().copy_from_slice(buf.cells());
        surf.present_all();
        let (line, col) = ed.cursor();
        let shape = if ed.mode() == Mode::Insert { CURSOR_BAR } else { CURSOR_BLOCK };
        surf.set_cursor(line.saturating_sub(top) as u32, col as u32, shape, true);

        match con.next_event() {
            Some(ConsoleEvent::Char('\n')) => ed.on_special(Special::Enter),
            Some(ConsoleEvent::Char('\t')) => ed.on_special(Special::Tab),
            Some(ConsoleEvent::Char(c)) => ed.on_char(c),
            Some(ConsoleEvent::Key { code, down: true, .. }) => match code {
                keys::ESC => ed.on_special(Special::Esc),
                keys::ENTER => ed.on_special(Special::Enter),
                keys::BACKSPACE => ed.on_special(Special::Backspace),
                keys::UP => ed.on_special(Special::Up),
                keys::DOWN => ed.on_special(Special::Down),
                keys::LEFT => ed.on_special(Special::Left),
                keys::RIGHT => ed.on_special(Special::Right),
                _ => {}
            },
            Some(ConsoleEvent::Resize { cols: c, rows: r }) if (c, r) != (cols, rows) => {
                (cols, rows) = (c, r);
                surf = match con.open_surface(cols, rows) {
                    Ok(s) => s,
                    Err(_) => return 1,
                };
                buf.resize(cols as usize, rows as usize);
            }
            Some(ConsoleEvent::CloseRequested) | None => break,
            Some(_) => {}
        }

        // Perform the engine's side effects (it gates :q on dirtiness itself).
        let mut quit = false;
        for eff in ed.take_effects() {
            let save = |ed: &mut Editor, p: Option<String>| -> bool {
                let target = p.unwrap_or_else(|| path.clone());
                match fs::write(&target, ed.text().as_bytes()) {
                    Ok(()) => {
                        ed.mark_saved();
                        ed.set_status(format!("\"{target}\" written"));
                        true
                    }
                    Err(st) => {
                        ed.set_status(format!("E: write failed ({st})"));
                        false
                    }
                }
            };
            match eff {
                Effect::Save(p) => {
                    save(&mut ed, p);
                }
                Effect::SaveQuit(p) => quit = save(&mut ed, p),
                Effect::Quit | Effect::ForceQuit => quit = true,
            }
        }
        if quit {
            break;
        }
    }
    surf.close();
    0
}

app!(main);
