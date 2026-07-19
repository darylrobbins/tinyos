//! top — live thread/process/memory viewer over the process-control
//! protocol (abi::proc). Select with up/down, k kills, q quits.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;

use abi::console::INPUT_MODE_KEYS;
use abi::keys;
use abi::proc::{STATE_BLOCKED, STATE_READY, STATE_RUNNING};
use textui::{CellBuffer, Style, ATTR_BOLD, ATTR_DIM, ATTR_INVERSE};
use tinyos_app::wait::sleep_us;
use tinyos_app::{app, entry, proc, ConsoleEvent, Env};

const ACC: u32 = abi::tokens::ACC;
const TX2: u32 = abi::tokens::TX2;

fn state_str(s: u32) -> &'static str {
    match s {
        STATE_READY => "ready",
        STATE_RUNNING => "run",
        STATE_BLOCKED => "block",
        _ => "exit",
    }
}

fn main(_env: Env) -> i32 {
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
    let mut sel = 0usize;
    let mut note = alloc::string::String::new();

    'outer: loop {
        let info = proc::sysinfo().ok();
        let (threads, procs) = proc::ps().unwrap_or_default();
        sel = sel.min(threads.len().saturating_sub(1));

        buf.clear(Style::default());
        if let Some(i) = &info {
            let up = i.uptime_us / 1_000_000;
            buf.put_str(
                1,
                0,
                &format!(
                    "up {}:{:02}:{:02}  heap {}/{} MiB  pool {}/{} MiB",
                    up / 3600,
                    up / 60 % 60,
                    up % 60,
                    i.heap_used >> 20,
                    (i.heap_used + i.heap_free) >> 20,
                    (i.pool_total - i.pool_free) >> 20,
                    i.pool_total >> 20,
                ),
                Style::new(ACC, 0, ATTR_BOLD),
            );
        }
        buf.put_str(
            1,
            1,
            &format!("{:>4} {:<10} {:>3} {:<6} MEM", "ID", "NAME", "CPU", "STATE"),
            Style::new(TX2, 0, ATTR_DIM),
        );
        let list_rows = rows as usize - 3;
        for (i, t) in threads.iter().take(list_rows).enumerate() {
            let mem = procs
                .iter()
                .find(|p| p.tid == t.id)
                .map(|p| format!("{} KiB", p.mem >> 10))
                .unwrap_or_default();
            let style = if i == sel {
                Style::new(ACC, 0, ATTR_INVERSE)
            } else {
                Style::fg(0)
            };
            buf.put_str(
                1,
                2 + i,
                &format!(
                    "{:>4} {:<10} {:>3} {:<6} {}",
                    t.id,
                    &t.name[..t.name.len().min(10)],
                    t.cpu,
                    state_str(t.state),
                    mem
                ),
                style,
            );
        }
        let bar = format!(" {note}  up/down: select  k: kill  q: quit ");
        buf.put_str(0, rows as usize - 1, &bar, Style::new(TX2, 0, ATTR_INVERSE));
        surf.cells().copy_from_slice(buf.cells());
        surf.present_all();

        // ~2 Hz refresh, polling input between frames.
        for _ in 0..10 {
            while let Some(ev) = con.poll_event() {
                match ev {
                    ConsoleEvent::Char('q') | ConsoleEvent::CloseRequested => break 'outer,
                    ConsoleEvent::Char('k') => {
                        if let Some(t) = threads.get(sel) {
                            note = match proc::kill(t.id) {
                                Ok(()) => format!("killed {}", t.id),
                                Err(st) => format!("kill {} failed ({st})", t.id),
                            };
                        }
                    }
                    ConsoleEvent::Key { code: keys::UP, down: true, .. } => {
                        sel = sel.saturating_sub(1);
                    }
                    ConsoleEvent::Key { code: keys::DOWN, down: true, .. } => {
                        sel += 1;
                    }
                    ConsoleEvent::Resize { cols: c, rows: r } if (c, r) != (cols, rows) => {
                        (cols, rows) = (c, r);
                        surf = match con.open_surface(cols, rows) {
                            Ok(s) => s,
                            Err(_) => break 'outer,
                        };
                        buf.resize(cols as usize, rows as usize);
                    }
                    _ => {}
                }
            }
            sleep_us(50_000);
        }
    }
    surf.close();
    0
}

app!(main);
tinyos_app::declare_caps!(b"console\nwindow\nproc");
