//! termcore: pure line-world model for the userspace terminal (terminal
//! spec, Layer 3). Owns scrollback, the line editor, and the console
//! protocol v1 server state machine — no I/O, so it is host-testable
//! (`cargo test -p termcore`). `apps/terminal` drives it.
//!
//! This is the line-world subset of the in-kernel terminal
//! (`kernel/src/term/mod.rs`); SURFACE_*/LIVE_* handling is intentionally
//! dropped here (SP1b will add a surface-aware sibling or extend this).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use abi::console::{
    INPUT_MODE_KEYS, INPUT_MODE_LINES, OP_CHAR, OP_CLEAR, OP_HELLO, OP_HELLO_ACK, OP_INPUT_LINE,
    OP_KEY, OP_RESIZE, OP_SET_FOREGROUND, OP_SET_INPUT_MODE, OP_SET_PROMPT, OP_WRITE,
    OP_WRITE_STYLED,
};
use abi::keys;

/// Default (unstyled `OP_WRITE`) foreground color — matches the in-kernel
/// terminal's `FG` (`rgb(0xe8, 0xec, 0xf2)`, i.e. `argb(255, ..)`).
const FG: u32 = 0xFFE8_ECF2;
/// Dimmed color used for echoed input lines — matches the kernel's `DIM`
/// (`rgb(0x5f, 0x68, 0x79)`).
const DIM: u32 = 0xFF5F_6879;
/// Scrollback retention cap; oldest lines evict first.
const SCROLLBACK_CAP: usize = 400;

/// One frozen scrollback line.
pub struct Line {
    pub text: String,
    pub color: u32,
}

/// Pure state of the terminal's line world: scrollback, the line editor,
/// and the console-protocol v1 server state machine. No I/O — the host
/// (`apps/terminal`) feeds it console-message bytes and keystrokes, and
/// reads back render state and outbound console-message bytes.
pub struct Term {
    scrollback: VecDeque<Line>,
    prompt: Vec<(String, u32)>,
    input: String,
    cursor: usize,
    partial: String,
    partial_color: u32,
    mode: u32,
    foreground_tid: u32,
    cols: usize,
    rows: usize,
    out: Vec<Vec<u8>>,
    dirty: bool,
}

impl Default for Term {
    fn default() -> Self {
        Self::new()
    }
}

impl Term {
    pub fn new() -> Self {
        Term {
            scrollback: VecDeque::new(),
            prompt: Vec::new(),
            input: String::new(),
            cursor: 0,
            partial: String::new(),
            partial_color: FG,
            mode: INPUT_MODE_LINES,
            foreground_tid: 0,
            cols: 0,
            rows: 0,
            out: Vec::new(),
            dirty: false,
        }
    }

    fn freeze(&mut self, text: String, color: u32) {
        self.scrollback.push_back(Line { text, color });
        while self.scrollback.len() > SCROLLBACK_CAP {
            self.scrollback.pop_front();
        }
        self.dirty = true;
    }

    /// Feed one inbound console message from the child (bytes only; SP1a
    /// ignores any moved handles). Updates scrollback/prompt/mode/foreground,
    /// may queue a `HELLO_ACK`. Sets `dirty` on any visible change.
    pub fn on_console_msg(&mut self, bytes: &[u8]) {
        if bytes.len() < 4 {
            return;
        }
        let op = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        match op {
            OP_WRITE => {
                self.partial_color = FG;
                if let Ok(s) = core::str::from_utf8(&bytes[4..]) {
                    for ch in s.chars() {
                        if ch == '\n' {
                            let text = core::mem::take(&mut self.partial);
                            self.freeze(text, FG);
                        } else {
                            self.partial.push(ch);
                            self.dirty = true;
                        }
                    }
                }
            }
            OP_WRITE_STYLED if bytes.len() >= 8 => {
                let fg = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                self.partial_color = fg;
                if let Ok(s) = core::str::from_utf8(&bytes[8..]) {
                    for ch in s.chars() {
                        if ch == '\n' {
                            let text = core::mem::take(&mut self.partial);
                            self.freeze(text, fg);
                        } else {
                            self.partial.push(ch);
                            self.dirty = true;
                        }
                    }
                }
            }
            OP_CLEAR => {
                self.scrollback.clear();
                self.dirty = true;
            }
            OP_SET_PROMPT if bytes.len() >= 8 => {
                let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
                let mut spans = Vec::with_capacity(count);
                let mut o = 8usize;
                for _ in 0..count {
                    let (Some(fg), Some(len)) = (
                        bytes.get(o..o + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap())),
                        bytes
                            .get(o + 4..o + 8)
                            .map(|c| u32::from_le_bytes(c.try_into().unwrap()) as usize),
                    ) else {
                        break;
                    };
                    o += 8;
                    let Some(txt) = bytes.get(o..o + len).and_then(|s| core::str::from_utf8(s).ok())
                    else {
                        break;
                    };
                    spans.push((String::from(txt), fg));
                    o += len;
                }
                self.prompt = spans;
                self.dirty = true;
            }
            OP_HELLO => {
                let mut b = OP_HELLO_ACK.to_le_bytes().to_vec();
                b.extend_from_slice(&1u32.to_le_bytes()); // protocol v1
                b.extend_from_slice(&0u32.to_le_bytes()); // features: none yet
                self.out.push(b);
            }
            OP_SET_INPUT_MODE if bytes.len() >= 8 => {
                self.mode = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
            }
            OP_SET_FOREGROUND if bytes.len() >= 8 => {
                self.foreground_tid = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
            }
            // SURFACE_*/LIVE_* and anything else are out of scope for the
            // line-world subset (SP1a) — ignored.
            _ => {}
        }
    }

    fn is_raw(&self) -> bool {
        self.mode == INPUT_MODE_KEYS
    }

    fn byte_index(&self, col: usize) -> usize {
        self.input.char_indices().nth(col).map(|(i, _)| i).unwrap_or(self.input.len())
    }

    /// A typed character from the window. LINES: local edit; KEYS: queue
    /// `OP_CHAR`.
    pub fn on_char(&mut self, c: char) {
        if self.is_raw() {
            let mut b = OP_CHAR.to_le_bytes().to_vec();
            b.extend_from_slice(&(c as u32).to_le_bytes());
            self.out.push(b);
            return;
        }
        if c == '\n' {
            let prompt_text: String = if !self.prompt.is_empty() {
                self.prompt.iter().map(|(t, _)| t.as_str()).collect()
            } else {
                self.partial.clone()
            };
            let text = self.input.clone();
            let echo = alloc::format!("{prompt_text}{text}");
            self.freeze(echo, DIM);
            let mut b = OP_INPUT_LINE.to_le_bytes().to_vec();
            b.extend_from_slice(text.as_bytes());
            self.out.push(b);
            self.input.clear();
            self.cursor = 0;
            self.dirty = true;
        } else {
            let bc = self.byte_index(self.cursor);
            self.input.insert(bc, c);
            self.cursor += 1;
            self.dirty = true;
        }
    }

    /// A non-char key (backspace/left/right/enter handled; others -> `OP_KEY`
    /// in KEYS mode).
    pub fn on_key(&mut self, code: u16) {
        if self.is_raw() {
            let mut b = OP_KEY.to_le_bytes().to_vec();
            b.extend_from_slice(&code.to_le_bytes());
            b.push(1); // down
            b.push(0); // mods
            self.out.push(b);
            return;
        }
        match code {
            keys::BACKSPACE => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    let bc = self.byte_index(self.cursor);
                    self.input.remove(bc);
                    self.dirty = true;
                }
            }
            keys::LEFT => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.dirty = true;
                }
            }
            keys::RIGHT => {
                let len = self.input.chars().count();
                if self.cursor < len {
                    self.cursor += 1;
                    self.dirty = true;
                }
            }
            keys::ENTER => self.on_char('\n'),
            _ => {}
        }
    }

    /// Set terminal size in cells; queues `OP_RESIZE` if changed.
    pub fn set_size(&mut self, cols: usize, rows: usize) {
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let mut b = OP_RESIZE.to_le_bytes().to_vec();
        b.extend_from_slice(&(cols as u32).to_le_bytes());
        b.extend_from_slice(&(rows as u32).to_le_bytes());
        self.out.push(b);
    }

    /// The foreground child tid the app should target for Ctrl+C (0 = none).
    pub fn foreground_tid(&self) -> u32 {
        self.foreground_tid
    }

    /// Drain queued outbound messages (each a full console-protocol frame).
    pub fn take_outbound(&mut self) -> Vec<Vec<u8>> {
        core::mem::take(&mut self.out)
    }

    /// True since the last render; cleared by `clear_dirty`.
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Read models for rendering (Task 3 lays these out with the mono atlas).
    pub fn scrollback(&self) -> impl Iterator<Item = &Line> {
        self.scrollback.iter()
    }

    pub fn prompt(&self) -> &[(String, u32)] {
        &self.prompt
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi::console::{
        OP_CHAR, OP_INPUT_LINE, OP_RESIZE, OP_SET_FOREGROUND, OP_SET_PROMPT, OP_WRITE_STYLED,
        INPUT_MODE_KEYS,
    };

    fn styled(fg: u32, s: &str) -> alloc::vec::Vec<u8> {
        let mut b = OP_WRITE_STYLED.to_le_bytes().to_vec();
        b.extend_from_slice(&fg.to_le_bytes());
        b.extend_from_slice(s.as_bytes());
        b
    }

    fn set_prompt_msg(spans: &[(u32, &str)]) -> alloc::vec::Vec<u8> {
        let mut b = OP_SET_PROMPT.to_le_bytes().to_vec();
        b.extend_from_slice(&(spans.len() as u32).to_le_bytes());
        for (fg, text) in spans {
            b.extend_from_slice(&fg.to_le_bytes());
            b.extend_from_slice(&(text.len() as u32).to_le_bytes());
            b.extend_from_slice(text.as_bytes());
        }
        b
    }

    fn set_input_mode_msg(mode: u32) -> alloc::vec::Vec<u8> {
        let mut b = abi::console::OP_SET_INPUT_MODE.to_le_bytes().to_vec();
        b.extend_from_slice(&mode.to_le_bytes());
        b
    }

    fn set_foreground_msg(tid: u32) -> alloc::vec::Vec<u8> {
        let mut b = OP_SET_FOREGROUND.to_le_bytes().to_vec();
        b.extend_from_slice(&tid.to_le_bytes());
        b
    }

    #[test]
    fn write_styled_freezes_line_on_newline() {
        let mut t = Term::new();
        t.on_console_msg(&styled(0xAABBCC, "hello\n"));
        let lines: alloc::vec::Vec<_> = t.scrollback().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "hello");
        assert_eq!(lines[0].color, 0xAABBCC);
    }

    #[test]
    fn enter_queues_input_line_and_echoes() {
        let mut t = Term::new();
        for c in "ls /apps".chars() {
            t.on_char(c);
        }
        t.on_char('\n');
        let out = t.take_outbound();
        assert_eq!(out.len(), 1);
        let op = u32::from_le_bytes(out[0][0..4].try_into().unwrap());
        assert_eq!(op, OP_INPUT_LINE);
        assert_eq!(&out[0][4..], b"ls /apps");
        assert_eq!(t.input(), "");
    }

    #[test]
    fn set_prompt_parses_colored_spans() {
        let mut t = Term::new();
        t.on_console_msg(&set_prompt_msg(&[(0x11_2233, "meridian"), (0x44_5566, "> ")]));
        let spans = t.prompt();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0], (String::from("meridian"), 0x11_2233));
        assert_eq!(spans[1], (String::from("> "), 0x44_5566));
    }

    #[test]
    fn backspace_left_right_edit_input() {
        let mut t = Term::new();
        for c in "abc".chars() {
            t.on_char(c);
        }
        assert_eq!(t.input(), "abc");
        assert_eq!(t.cursor(), 3);

        t.on_key(abi::keys::LEFT);
        assert_eq!(t.cursor(), 2);
        t.on_key(abi::keys::LEFT);
        assert_eq!(t.cursor(), 1);

        t.on_key(abi::keys::BACKSPACE);
        assert_eq!(t.input(), "bc");
        assert_eq!(t.cursor(), 0);

        // Left at column 0 is a no-op.
        t.on_key(abi::keys::LEFT);
        assert_eq!(t.cursor(), 0);

        t.on_key(abi::keys::RIGHT);
        assert_eq!(t.cursor(), 1);
        t.on_key(abi::keys::RIGHT);
        t.on_key(abi::keys::RIGHT);
        // Right at end of input is a no-op.
        assert_eq!(t.cursor(), 2);
    }

    #[test]
    fn keys_mode_char_queues_op_char() {
        let mut t = Term::new();
        t.on_console_msg(&set_input_mode_msg(INPUT_MODE_KEYS));
        t.on_char('x');
        let out = t.take_outbound();
        assert_eq!(out.len(), 1);
        let op = u32::from_le_bytes(out[0][0..4].try_into().unwrap());
        assert_eq!(op, OP_CHAR);
        let c = u32::from_le_bytes(out[0][4..8].try_into().unwrap());
        assert_eq!(c, 'x' as u32);
        // Raw mode never touches the local input buffer.
        assert_eq!(t.input(), "");
    }

    #[test]
    fn set_size_queues_resize_only_on_change() {
        let mut t = Term::new();
        t.set_size(80, 24);
        let out = t.take_outbound();
        assert_eq!(out.len(), 1);
        let op = u32::from_le_bytes(out[0][0..4].try_into().unwrap());
        assert_eq!(op, OP_RESIZE);

        // Same size again -> nothing queued.
        t.set_size(80, 24);
        assert!(t.take_outbound().is_empty());

        // Changed size -> queued again.
        t.set_size(100, 30);
        assert_eq!(t.take_outbound().len(), 1);
    }

    #[test]
    fn set_foreground_updates_foreground_tid() {
        let mut t = Term::new();
        assert_eq!(t.foreground_tid(), 0);
        t.on_console_msg(&set_foreground_msg(42));
        assert_eq!(t.foreground_tid(), 42);
    }

    #[test]
    fn scrollback_caps_at_400() {
        let mut t = Term::new();
        for i in 0..410 {
            t.on_console_msg(&styled(0, &alloc::format!("{i}\n")));
        }
        let lines: alloc::vec::Vec<_> = t.scrollback().collect();
        assert_eq!(lines.len(), 400);
        // Oldest 10 lines evicted; the retained window starts at "10".
        assert_eq!(lines[0].text, "10");
        assert_eq!(lines[399].text, "409");
    }
}
