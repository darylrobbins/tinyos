//! Console client: writes go to the terminal over the CONSOLE channel using
//! the console protocol (u32 opcode + payload).

use core::fmt::{self, Write};

use alloc::string::String;
use alloc::vec::Vec;

use crate::channel::{Channel, Msg};

use abi::console::*;

/// An event delivered by the terminal emulator (console protocol v1).
pub enum ConsoleEvent {
    /// A complete input line (LINES mode; no trailing newline).
    Line(String),
    /// A typed character (KEYS mode).
    Char(char),
    /// A key edge (KEYS mode). The emulator currently only reports downs.
    Key { code: u16, down: bool, mods: u8 },
    /// Terminal size in cells; sent once after start and on every change.
    Resize { cols: u32, rows: u32 },
    /// Protocol handshake reply.
    Hello { ver: u32, features: u32 },
    /// The user asked to close; the app should exit.
    CloseRequested,
}

pub struct Console {
    ch: Channel,
    cols: u32,
    rows: u32,
}

impl Console {
    pub fn new(ch: Channel) -> Self {
        Self { ch, cols: 0, rows: 0 }
    }

    pub fn write_bytes(&self, s: &[u8]) {
        let mut msg = Vec::with_capacity(4 + s.len());
        msg.extend_from_slice(&OP_WRITE.to_le_bytes());
        msg.extend_from_slice(s);
        let _ = self.ch.send(&msg, &[]);
    }

    /// Switch input delivery: `INPUT_MODE_LINES` (default; the emulator
    /// edits/echoes and delivers whole lines) or `INPUT_MODE_KEYS` (raw
    /// char/key events, no echo).
    pub fn set_input_mode(&self, mode: u32) {
        let mut msg = OP_SET_INPUT_MODE.to_le_bytes().to_vec();
        msg.extend_from_slice(&mode.to_le_bytes());
        let _ = self.ch.send(&msg, &[]);
    }

    /// Last terminal size in cells, or (0, 0) before the first Resize event.
    pub fn size(&self) -> (u32, u32) {
        (self.cols, self.rows)
    }

    /// Block for the next console event. `None` means the terminal went away.
    pub fn next_event(&mut self) -> Option<ConsoleEvent> {
        loop {
            let msg = self.ch.recv().ok()?;
            if let Some(ev) = decode(&msg) {
                if let ConsoleEvent::Resize { cols, rows } = ev {
                    self.cols = cols;
                    self.rows = rows;
                }
                return Some(ev);
            }
        }
    }

    /// Block for one input line (LINES mode). `None` if the terminal went
    /// away. Other events (resizes etc.) are absorbed along the way.
    pub fn read_line(&mut self) -> Option<String> {
        loop {
            match self.next_event()? {
                ConsoleEvent::Line(s) => return Some(s),
                ConsoleEvent::CloseRequested => return None,
                _ => {}
            }
        }
    }
}

fn decode(m: &Msg) -> Option<ConsoleEvent> {
    if m.bytes.len() < 4 {
        return None;
    }
    let b = &m.bytes;
    let u32at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    match u32at(0) {
        OP_INPUT_LINE => Some(ConsoleEvent::Line(
            String::from_utf8_lossy(&b[4..]).into_owned(),
        )),
        OP_CHAR if b.len() >= 8 => char::from_u32(u32at(4)).map(ConsoleEvent::Char),
        OP_KEY if b.len() >= 8 => Some(ConsoleEvent::Key {
            code: u16::from_le_bytes(b[4..6].try_into().unwrap()),
            down: b[6] != 0,
            mods: b[7],
        }),
        OP_RESIZE if b.len() >= 12 => Some(ConsoleEvent::Resize {
            cols: u32at(4),
            rows: u32at(8),
        }),
        OP_HELLO_ACK if b.len() >= 12 => Some(ConsoleEvent::Hello {
            ver: u32at(4),
            features: u32at(8),
        }),
        OP_CLOSE_REQ => Some(ConsoleEvent::CloseRequested),
        _ => None,
    }
}

/// Block for one line of input on the process console (stdin, in effect).
/// `None` if the process has no console or the terminal went away.
pub fn read_line() -> Option<String> {
    crate::entry::console()?.read_line()
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_bytes(s.as_bytes());
        Ok(())
    }
}

/// `println!`-style macros routed through the process console. Available
/// after `entry` stores the console handle.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        if let Some(c) = $crate::entry::console() {
            let _ = write!(c, $($arg)*);
        }
    }};
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        if let Some(c) = $crate::entry::console() {
            let _ = writeln!(c, $($arg)*);
        }
    }};
}
