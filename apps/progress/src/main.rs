//! Live-region demo (terminal spec M4): streams log lines into scrollback
//! while animating a spinner + progress bar pinned to the bottom — the
//! Claude Code / Ink pattern, without a single escape sequence.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use textui::{CellBuffer, Style, ATTR_BOLD, ATTR_DIM};
use tinyos_app::wait::sleep_us;
use tinyos_app::{app, entry, println, ConsoleEvent, Env};

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const STEPS: usize = 24;
const ACC: u32 = abi::tokens::ACC;
const TX2: u32 = abi::tokens::TX2;

fn main(_env: Env) -> i32 {
    let con = entry::console().expect("no console");
    // Wait for the startup Resize so the terminal width is known.
    loop {
        match con.next_event() {
            Some(ConsoleEvent::Resize { .. }) => break,
            Some(_) => continue,
            None => return 1,
        }
    }
    let Ok(mut live) = con.open_live(2) else { return 1 };
    let mut buf = CellBuffer::new(live.cols as usize, 2);

    println!("starting 4 build jobs...");
    for step in 0..=STEPS {
        if step > 0 && step % 6 == 0 {
            println!("job {} finished", step / 6);
        }
        let pct = step * 100 / STEPS;
        buf.clear(Style::default());
        let spin = SPINNER[step % SPINNER.len()];
        buf.put(1, 0, spin, Style::new(ACC, 0, ATTR_BOLD));
        let end = buf.put_str(3, 0, &format!("building... {pct:>3}%"), Style::fg(0));
        buf.put_str(end + 2, 0, "(lines scroll above)", Style::new(TX2, 0, ATTR_DIM));
        // Bar on row 1.
        let width = (buf.cols - 4).max(4);
        let filled = width * step / STEPS;
        for i in 0..width {
            let (c, st) = if i < filled {
                ('█', Style::fg(ACC))
            } else {
                ('░', Style::new(TX2, 0, ATTR_DIM))
            };
            buf.put(2 + i, 1, c, st);
        }
        live.cells().copy_from_slice(buf.cells());
        live.present_all();
        sleep_us(160_000);
    }
    live.close();
    println!("all jobs done.");
    0
}

app!(main);

// Prints and drives a console live-region (open_live) — console only.
tinyos_app::declare_caps!(b"console");
