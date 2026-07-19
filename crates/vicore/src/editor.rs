//! The modal vi editor engine.
//!
//! [`Editor`] consumes semantic input events and mutates an in-memory
//! [`Buffer`]. It knows nothing about pixels or scancodes: the host translates
//! keys into [`Editor::on_char`] / [`Editor::on_special`] / [`Editor::on_ctrl`]
//! and renders the exposed state. File I/O and quitting are surfaced as
//! [`Effect`]s the host performs.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::buffer::{order, Buffer, Pos};

/// Editing mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    /// Charwise visual selection.
    Visual,
    /// Linewise visual selection.
    VisualLine,
    /// Typing an ex command after `:`.
    Command,
    /// Typing a search pattern after `/` or `?`.
    Search,
}

/// Non-character key events the host forwards to the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Special {
    Esc,
    Enter,
    Backspace,
    Tab,
    Left,
    Right,
    Up,
    Down,
}

/// A side effect the host must perform (the engine cannot touch files or windows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Write the buffer. `Some(path)` = `:w <path>`, `None` = write current file.
    Save(Option<String>),
    /// `:q` — close if there are no unsaved changes.
    Quit,
    /// `:q!` — close discarding changes.
    ForceQuit,
    /// Write then close (`:wq` / `:x`).
    SaveQuit(Option<String>),
}

/// A yank/delete register: text plus whether it was captured linewise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Register {
    text: String,
    linewise: bool,
}

#[derive(Clone)]
struct Snapshot {
    buf: Buffer,
    cursor: Pos,
}

/// Raw input for `.` repeat replay.
#[derive(Clone, Copy)]
enum Ev {
    Char(char),
    Special(Special),
    Ctrl(char),
}

/// A pending motion/find awaiting its argument character.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Await {
    None,
    /// `f F t T` waiting for the target char; bool = forward, bool2 = till.
    Find { forward: bool, till: bool },
    /// `r` waiting for the replacement char.
    Replace,
    /// `m` waiting for the mark letter.
    Mark,
    /// `` ` `` or `'` waiting for the mark letter; bool = linewise (`'`).
    Goto { linewise: bool },
    /// `"` waiting for the register letter.
    Register,
    /// `g` waiting for the second key (`gg`).
    G,
}

pub struct Editor {
    buf: Buffer,
    cursor: Pos,
    /// Sticky column for vertical motion (`j`/`k`), `usize::MAX` = end-of-line.
    desired_col: usize,
    mode: Mode,

    count: Option<usize>,
    /// Whether the current count was typed explicitly (affects `G`).
    explicit_count: bool,
    pending_op: Option<char>,
    /// Count typed after the operator (e.g. the `3` in `d3w`).
    op_count: Option<usize>,
    await_: Await,
    active_register: Option<char>,

    visual_anchor: Pos,
    visual_linewise: bool,

    registers: BTreeMap<char, Register>,
    unnamed: Register,

    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// True while an insert session is open (undo already snapshotted).
    in_insert_change: bool,

    last_find: Option<(bool, bool, char)>, // forward, till, char
    last_search: Option<(String, bool)>,   // pattern, forward

    marks: BTreeMap<char, Pos>,
    cmdline: String,

    // `.` repeat: capture the events of the current change, promote on completion.
    cur_events: Vec<Ev>,
    last_change: Vec<Ev>,
    replaying: bool,
    version: u64, // bumped on every buffer mutation

    view_top: usize,
    view_rows: usize,

    effects: Vec<Effect>,
    status: String,
    dirty: bool,
}

impl Editor {
    pub fn new(text: &str) -> Self {
        Self {
            buf: Buffer::from_str(text),
            cursor: Pos::new(0, 0),
            desired_col: 0,
            mode: Mode::Normal,
            count: None,
            explicit_count: false,
            pending_op: None,
            op_count: None,
            await_: Await::None,
            active_register: None,
            visual_anchor: Pos::new(0, 0),
            visual_linewise: false,
            registers: BTreeMap::new(),
            unnamed: Register::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            in_insert_change: false,
            last_find: None,
            last_search: None,
            marks: BTreeMap::new(),
            cmdline: String::new(),
            cur_events: Vec::new(),
            last_change: Vec::new(),
            replaying: false,
            version: 0,
            view_top: 0,
            view_rows: 24,
            effects: Vec::new(),
            status: String::new(),
            dirty: false,
        }
    }

    // ---- Read-only accessors for the host/renderer ----

    pub fn lines(&self) -> &[String] {
        self.buf.lines()
    }
    pub fn line_count(&self) -> usize {
        self.buf.line_count()
    }
    /// 0-based `(line, col)` cursor.
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor.line, self.cursor.col)
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn set_status(&mut self, s: String) {
        self.status = s;
    }
    /// The buffer serialized for saving (trailing newline included).
    pub fn text(&self) -> String {
        self.buf.to_string()
    }
    /// The command/search line being typed, including its `:` / `/` / `?` prefix.
    pub fn command_line(&self) -> Option<String> {
        match self.mode {
            Mode::Command => Some(alloc::format!(":{}", self.cmdline)),
            Mode::Search => {
                let p = if self.last_search.as_ref().map(|s| s.1).unwrap_or(true) {
                    '/'
                } else {
                    '?'
                };
                Some(alloc::format!("{p}{}", self.cmdline))
            }
            _ => None,
        }
    }
    /// Normalized visual selection `(start, end, linewise)`, end inclusive.
    pub fn visual_range(&self) -> Option<(Pos, Pos, bool)> {
        match self.mode {
            Mode::Visual => {
                let (a, b) = order(self.visual_anchor, self.cursor);
                Some((a, b, false))
            }
            Mode::VisualLine => {
                let (a, b) = order(self.visual_anchor, self.cursor);
                Some((a, b, true))
            }
            _ => None,
        }
    }
    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            Mode::Normal => "",
            Mode::Insert => "-- INSERT --",
            Mode::Visual => "-- VISUAL --",
            Mode::VisualLine => "-- VISUAL LINE --",
            Mode::Command | Mode::Search => "",
        }
    }

    /// Drain queued host effects (file writes, quit).
    pub fn take_effects(&mut self) -> Vec<Effect> {
        core::mem::take(&mut self.effects)
    }

    /// Mark the buffer clean after a successful host save.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    // ---- Viewport ----

    pub fn set_view_rows(&mut self, rows: usize) {
        self.view_rows = rows.max(1);
    }
    /// Adjust the viewport so the cursor is visible; returns the new top line.
    pub fn scroll_into_view(&mut self) -> usize {
        if self.cursor.line < self.view_top {
            self.view_top = self.cursor.line;
        } else if self.cursor.line >= self.view_top + self.view_rows {
            self.view_top = self.cursor.line + 1 - self.view_rows;
        }
        self.view_top
    }
    pub fn view_top(&self) -> usize {
        self.view_top
    }

    // ---- Input entry points ----

    pub fn on_char(&mut self, c: char) {
        if !self.replaying {
            self.cur_events.push(Ev::Char(c));
        }
        match self.mode {
            Mode::Insert => self.insert_char(c),
            Mode::Command | Mode::Search => self.cmdline_char(c),
            Mode::Normal | Mode::Visual | Mode::VisualLine => self.normal_char(c),
        }
    }

    pub fn on_special(&mut self, s: Special) {
        if !self.replaying {
            self.cur_events.push(Ev::Special(s));
        }
        match self.mode {
            Mode::Insert => self.insert_special(s),
            Mode::Command | Mode::Search => self.cmdline_special(s),
            _ => self.normal_special(s),
        }
    }

    pub fn on_ctrl(&mut self, c: char) {
        if !self.replaying {
            self.cur_events.push(Ev::Ctrl(c));
        }
        match c.to_ascii_lowercase() {
            'r' if self.mode == Mode::Normal => {
                self.redo();
                self.end_command(false);
            }
            'f' => self.page(self.view_rows.saturating_sub(2) as isize),
            'b' => self.page(-((self.view_rows.saturating_sub(2)) as isize)),
            'd' => self.page((self.view_rows / 2) as isize),
            'u' => self.page(-((self.view_rows / 2) as isize)),
            _ => {}
        }
    }

    // ---- `.` repeat bookkeeping ----

    /// Called when a normal-mode command sequence resolves. `changed` = whether
    /// it mutated the buffer (and thus is repeatable with `.`).
    fn end_command(&mut self, changed: bool) {
        if !self.replaying && changed && self.mode == Mode::Normal {
            self.last_change = core::mem::take(&mut self.cur_events);
        }
        if self.mode == Mode::Normal {
            self.cur_events.clear();
        }
        self.count = None;
        self.explicit_count = false;
        self.pending_op = None;
        self.op_count = None;
        self.await_ = Await::None;
        self.active_register = None;
    }

    fn repeat_last(&mut self, count: Option<usize>) {
        if self.last_change.is_empty() {
            return;
        }
        let events = self.last_change.clone();
        let n = count.unwrap_or(1);
        self.replaying = true;
        for _ in 0..n {
            for ev in &events {
                match *ev {
                    Ev::Char(c) => self.on_char(c),
                    Ev::Special(s) => self.on_special(s),
                    Ev::Ctrl(c) => self.on_ctrl(c),
                }
            }
        }
        self.replaying = false;
    }

    // ---- Undo ----

    fn push_undo(&mut self) {
        self.undo.push(Snapshot {
            buf: self.buf.clone(),
            cursor: self.cursor,
        });
        self.redo.clear();
    }
    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(Snapshot {
                buf: self.buf.clone(),
                cursor: self.cursor,
            });
            self.buf = prev.buf;
            self.cursor = self.clamp(prev.cursor);
            self.version += 1;
            self.dirty = true;
        }
    }
    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(Snapshot {
                buf: self.buf.clone(),
                cursor: self.cursor,
            });
            self.buf = next.buf;
            self.cursor = self.clamp(next.cursor);
            self.version += 1;
            self.dirty = true;
        }
    }

    fn touch(&mut self) {
        self.version += 1;
        self.dirty = true;
    }

    // ---- Cursor helpers ----

    fn last_line(&self) -> usize {
        self.buf.line_count().saturating_sub(1)
    }
    /// Max column for the cursor on `line` given the mode (normal clamps to the
    /// last char; insert/visual may sit one past the end).
    fn max_col(&self, line: usize) -> usize {
        let len = self.buf.line_len(line);
        if matches!(self.mode, Mode::Insert | Mode::Visual | Mode::VisualLine) {
            len
        } else {
            len.saturating_sub(1).min(len)
        }
    }
    fn clamp(&self, mut p: Pos) -> Pos {
        p.line = p.line.min(self.last_line());
        let maxc = self.max_col(p.line);
        p.col = p.col.min(maxc);
        p
    }
    fn clamp_cursor(&mut self) {
        self.cursor = self.clamp(self.cursor);
    }
}

// Character classes for word motions.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Blank,
    Word,
    Punct,
}
fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Blank
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

mod motions;
mod normal;
mod ex;

#[cfg(test)]
mod tests;
