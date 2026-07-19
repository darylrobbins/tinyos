//! Console client: writes go to the terminal over the CONSOLE channel using
//! the console protocol (u32 opcode + payload).

use core::fmt::{self, Write};

use alloc::string::String;
use alloc::vec::Vec;

use crate::channel::{Channel, Msg};
use crate::syscall::{SIG_WRITABLE, ST_SHOULD_WAIT};

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
    /// Output buffer: `write_str` fragments (padding, args) accumulate here
    /// and flush per line, so one `println!` is one channel message instead
    /// of one per format fragment — a padded `ps` row is ~30 write_str calls
    /// and the channel caps at 64 messages, so unbuffered it truncated
    /// mid-row. Flushed on newline, on overflow, and before reading input.
    out: Vec<u8>,
}

const FLUSH_AT: usize = 8192;

impl Console {
    pub fn new(ch: Channel) -> Self {
        Self { ch, cols: 0, rows: 0, out: Vec::new() }
    }

    /// Buffer bytes for output; complete lines flush automatically.
    pub fn write_bytes(&mut self, s: &[u8]) {
        self.out.extend_from_slice(s);
        if self.out.len() >= FLUSH_AT || self.out.contains(&b'\n') {
            self.flush();
        }
    }

    /// Send any buffered output now, coalesced into as few messages as the
    /// channel byte cap allows. Call before blocking for input so an
    /// unterminated prompt is visible.
    pub fn flush(&mut self) {
        if self.out.is_empty() {
            return;
        }
        // Channel cap is 64 KiB/msg; stay well under with the opcode header.
        const CHUNK: usize = 32 * 1024;
        let out = core::mem::take(&mut self.out);
        for part in out.chunks(CHUNK) {
            self.send_chunk(part);
        }
    }

    fn send_chunk(&self, s: &[u8]) {
        let mut msg = Vec::with_capacity(4 + s.len());
        msg.extend_from_slice(&OP_WRITE.to_le_bytes());
        msg.extend_from_slice(s);
        // Bounded channel: block on WRITABLE and retry rather than dropping
        // (the terminal's pump frees space and wakes us).
        loop {
            match self.ch.send(&msg, &[]) {
                Ok(()) => return,
                Err(ST_SHOULD_WAIT) => {
                    let mut it = [crate::wait::WaitItem {
                        handle: self.ch.0,
                        want: SIG_WRITABLE,
                        observed: 0,
                    }];
                    let _ = crate::wait::wait_many(&mut it, u64::MAX);
                }
                Err(_) => return, // peer gone: drop silently
            }
        }
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
        self.flush(); // make any pending output (e.g. a prompt) visible first
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

    /// Non-blocking: the next pending console event, if any.
    pub fn poll_event(&mut self) -> Option<ConsoleEvent> {
        loop {
            let msg = self.ch.try_recv().ok()?;
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

/// A full-screen cell surface hosted by the terminal (console protocol
/// SURFACE_*): shared-memory cells, damage-tracked presents, cursor control.
/// While open, the terminal delivers raw Char/Key events and freezes
/// scrollback underneath (alt-screen semantics).
pub struct TextSurface {
    ch: Channel,
    pub cols: u32,
    pub rows: u32,
    va: u64,
    mem: u64,
}

impl Console {
    /// Open a `cols` x `rows` surface. Size it from a Resize event ([`Console::size`]).
    pub fn open_surface(&mut self, cols: u32, rows: u32) -> Result<TextSurface, u32> {
        use crate::syscall::*;
        let bytes = cols as u64 * rows as u64 * 16;
        let size = (bytes + 0xFFF) & !0xFFF;
        let mem = syscall1(SYS_MEMOBJ_CREATE, size).ok()?;
        let dup = syscall2(SYS_HANDLE_DUP, mem, RIGHTS_ALL as u64).ok()? as u32;
        let va = syscall3(SYS_MEMOBJ_MAP, mem, 0, size).ok()?;
        let mut msg = OP_SURFACE_OPEN.to_le_bytes().to_vec();
        msg.extend_from_slice(&cols.to_le_bytes());
        msg.extend_from_slice(&rows.to_le_bytes());
        self.ch.send(&msg, &[dup])?;
        Ok(TextSurface { ch: self.ch, cols, rows, va, mem })
    }
}

impl TextSurface {
    /// The shared cell grid (row-major, stride = cols).
    pub fn cells(&mut self) -> &mut [Cell] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.va as *mut Cell,
                (self.cols * self.rows) as usize,
            )
        }
    }

    /// Present a damage rect (in cells).
    pub fn present(&self, x: u32, y: u32, w: u32, h: u32) {
        let mut msg = OP_SURFACE_PRESENT.to_le_bytes().to_vec();
        for v in [x, y, w, h] {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        let _ = self.ch.send(&msg, &[]);
    }

    pub fn present_all(&self) {
        self.present(0, 0, self.cols, self.rows);
    }

    /// Place the terminal cursor (CURSOR_* shapes; visible = false hides it).
    pub fn set_cursor(&self, row: u32, col: u32, shape: u32, visible: bool) {
        let mut msg = OP_SURFACE_CURSOR.to_le_bytes().to_vec();
        for v in [row, col, shape, visible as u32] {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        let _ = self.ch.send(&msg, &[]);
    }

    /// Close the surface; the terminal restores scrollback.
    pub fn close(self) {
        let _ = self.ch.send(&OP_SURFACE_CLOSE.to_le_bytes(), &[]);
    }
}

impl Drop for TextSurface {
    fn drop(&mut self) {
        // Reclaim the VA range and the handle; runs after close(), and also
        // on silent replacement (resize re-open).
        use crate::syscall::*;
        syscall1(SYS_MEMOBJ_UNMAP, self.va);
        syscall1(SYS_HANDLE_CLOSE, self.mem);
    }
}

/// A bottom-pinned live cell region (console protocol LIVE_*): `WRITE`d
/// lines keep scrolling above while the app redraws this area in place —
/// the Ink-style static/dynamic split. On close (or exit) the terminal
/// flattens the final frame into scrollback.
pub struct LiveRegion {
    ch: Channel,
    pub cols: u32,
    pub rows: u32,
    va: u64,
    mem: u64,
}

impl Console {
    /// Open a live region `rows` tall spanning the terminal width. Needs a
    /// Resize event first (the terminal sends one at startup) so the width
    /// is known; fails with ST_SHOULD_WAIT before that.
    pub fn open_live(&mut self, rows: u32) -> Result<LiveRegion, u32> {
        use crate::syscall::*;
        let cols = self.cols;
        if cols == 0 {
            return Err(ST_SHOULD_WAIT);
        }
        let size = (cols as u64 * rows as u64 * 16 + 0xFFF) & !0xFFF;
        let mem = syscall1(SYS_MEMOBJ_CREATE, size).ok()?;
        let dup = syscall2(SYS_HANDLE_DUP, mem, RIGHTS_ALL as u64).ok()? as u32;
        let va = syscall3(SYS_MEMOBJ_MAP, mem, 0, size).ok()?;
        let mut msg = OP_LIVE_OPEN.to_le_bytes().to_vec();
        msg.extend_from_slice(&rows.to_le_bytes());
        self.ch.send(&msg, &[dup])?;
        Ok(LiveRegion { ch: self.ch, cols, rows, va, mem })
    }
}

impl LiveRegion {
    /// The shared cell grid (row-major, stride = cols).
    pub fn cells(&mut self) -> &mut [Cell] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.va as *mut Cell,
                (self.cols * self.rows) as usize,
            )
        }
    }

    pub fn present_all(&self) {
        let mut msg = OP_SURFACE_PRESENT.to_le_bytes().to_vec();
        for v in [0, 0, self.cols, self.rows] {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        let _ = self.ch.send(&msg, &[]);
    }

    /// Close; the terminal flattens the last frame into scrollback.
    pub fn close(self) {
        let _ = self.ch.send(&OP_LIVE_CLOSE.to_le_bytes(), &[]);
    }
}

impl Drop for LiveRegion {
    fn drop(&mut self) {
        use crate::syscall::*;
        syscall1(SYS_MEMOBJ_UNMAP, self.va);
        syscall1(SYS_HANDLE_CLOSE, self.mem);
    }
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
