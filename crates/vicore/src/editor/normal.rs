//! Normal-, visual- and insert-mode key handling.

use alloc::string::{String, ToString};

use super::motions::{Motion, MotionKind};
use super::{Await, Editor, Mode, Register, Special};
use crate::buffer::{order, Pos};

impl Editor {
    fn cmd_count(&self) -> usize {
        self.count.unwrap_or(1).max(1)
    }
    fn motion_count(&self) -> usize {
        self.count.unwrap_or(1).max(1) * self.op_count.unwrap_or(1).max(1)
    }

    // ---- Normal / visual character dispatch ----

    pub(super) fn normal_char(&mut self, c: char) {
        if self.await_ != Await::None {
            self.resolve_await(c);
            return;
        }
        if c == '"' {
            self.await_ = Await::Register;
            return;
        }
        // Count accumulation ('0' is a motion only when no count is pending).
        if c.is_ascii_digit() && !(c == '0' && self.count.is_none() && self.op_count.is_none()) {
            let d = c as usize - '0' as usize;
            if self.pending_op.is_some() {
                self.op_count = Some(self.op_count.unwrap_or(0) * 10 + d);
            } else {
                self.count = Some(self.count.unwrap_or(0) * 10 + d);
                self.explicit_count = true;
            }
            return;
        }

        // Motions (may take an operator).
        if matches!(
            c,
            'h' | 'l' | 'j' | 'k' | '0' | '^' | '$' | '|' | 'G' | 'w' | 'W' | 'b' | 'B' | 'e'
                | 'E' | 'H' | 'M' | 'L' | ' '
        ) {
            self.apply_motion(c);
            return;
        }
        if matches!(c, 'f' | 'F' | 't' | 'T') {
            let forward = c == 'f' || c == 't';
            let till = c == 't' || c == 'T';
            self.await_ = Await::Find { forward, till };
            return;
        }
        if c == ';' || c == ',' {
            self.repeat_find(c == ',');
            return;
        }
        if c == 'g' {
            self.await_ = Await::G;
            return;
        }

        // In visual mode, operators act on the selection.
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            self.visual_char(c);
            return;
        }

        match c {
            'd' | 'c' | 'y' => {
                if self.pending_op == Some(c) {
                    // Doubled operator → linewise on the current line(s).
                    let n = self.motion_count();
                    let a = self.cursor.line;
                    let b = (a + n - 1).min(self.last_line());
                    self.op_lines(c, a, b);
                    self.end_command(c != 'y');
                } else if self.pending_op.is_none() {
                    self.pending_op = Some(c);
                }
            }
            'x' => {
                self.push_undo();
                let n = self.cmd_count();
                let start = self.cursor;
                let end = Pos::new(start.line, (start.col + n).min(self.len(start.line)));
                if end.col > start.col {
                    let text = self.buf.delete_range(start, end);
                    self.set_register(Register { text, linewise: false });
                    self.touch();
                }
                self.clamp_cursor();
                self.end_command(true);
            }
            'X' => {
                self.push_undo();
                let n = self.cmd_count();
                let end = self.cursor;
                let start = Pos::new(end.line, end.col.saturating_sub(n));
                if start.col < end.col {
                    let text = self.buf.delete_range(start, end);
                    self.set_register(Register { text, linewise: false });
                    self.cursor = start;
                    self.touch();
                }
                self.end_command(true);
            }
            'D' => {
                self.push_undo();
                let start = self.cursor;
                let end = Pos::new(start.line, self.len(start.line));
                let text = self.buf.delete_range(start, end);
                self.set_register(Register { text, linewise: false });
                self.clamp_cursor();
                self.touch();
                self.end_command(true);
            }
            'C' => {
                self.push_undo();
                let start = self.cursor;
                let end = Pos::new(start.line, self.len(start.line));
                let text = self.buf.delete_range(start, end);
                self.set_register(Register { text, linewise: false });
                self.touch();
                self.start_insert();
                self.end_command_insert();
            }
            's' => {
                self.push_undo();
                let n = self.cmd_count();
                let start = self.cursor;
                let end = Pos::new(start.line, (start.col + n).min(self.len(start.line)));
                if end.col > start.col {
                    let text = self.buf.delete_range(start, end);
                    self.set_register(Register { text, linewise: false });
                    self.touch();
                }
                self.start_insert();
                self.end_command_insert();
            }
            'S' => {
                self.push_undo();
                self.buf.set_line(self.cursor.line, String::new());
                self.cursor.col = 0;
                self.touch();
                self.start_insert();
                self.end_command_insert();
            }
            'r' => self.await_ = Await::Replace,
            '~' => {
                self.push_undo();
                let n = self.cmd_count();
                for _ in 0..n {
                    let (l, col) = (self.cursor.line, self.cursor.col);
                    if let Some(ch) = self.ch(l, col) {
                        let flipped: String = if ch.is_uppercase() {
                            ch.to_lowercase().collect()
                        } else {
                            ch.to_uppercase().collect()
                        };
                        self.buf.delete_char(l, col);
                        self.buf.insert_str(l, col, &flipped);
                        self.cursor.col = (col + 1).min(self.len(l).saturating_sub(1));
                    }
                }
                self.touch();
                self.end_command(true);
            }
            'J' => {
                self.push_undo();
                let n = self.cmd_count().max(2) - 1;
                let mut joined = false;
                for _ in 0..n {
                    let l = self.cursor.line;
                    if l + 1 >= self.buf.line_count() {
                        break;
                    }
                    let at = self.len(l);
                    // vim: collapse to a single space, dropping the next line's indent.
                    let next = self.buf.remove_line(l + 1);
                    let trimmed = next.trim_start();
                    let mut cur = self.buf.line(l).to_string();
                    if !cur.is_empty() && !trimmed.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(trimmed);
                    self.buf.set_line(l, cur);
                    self.cursor.col = at;
                    joined = true;
                }
                if joined {
                    self.touch();
                }
                self.clamp_cursor();
                self.end_command(joined);
            }
            'p' => {
                self.paste(true);
                self.end_command(true);
            }
            'P' => {
                self.paste(false);
                self.end_command(true);
            }
            'i' => {
                self.push_undo();
                self.start_insert();
                self.end_command_insert();
            }
            'I' => {
                self.push_undo();
                self.cursor.col = self.first_non_blank(self.cursor.line);
                self.start_insert();
                self.end_command_insert();
            }
            'a' => {
                self.push_undo();
                self.cursor.col = (self.cursor.col + 1).min(self.len(self.cursor.line));
                self.start_insert();
                self.end_command_insert();
            }
            'A' => {
                self.push_undo();
                self.cursor.col = self.len(self.cursor.line);
                self.start_insert();
                self.end_command_insert();
            }
            'o' => {
                self.push_undo();
                let l = self.cursor.line;
                self.buf.insert_line(l + 1, String::new());
                self.cursor = Pos::new(l + 1, 0);
                self.touch();
                self.start_insert();
                self.end_command_insert();
            }
            'O' => {
                self.push_undo();
                let l = self.cursor.line;
                self.buf.insert_line(l, String::new());
                self.cursor = Pos::new(l, 0);
                self.touch();
                self.start_insert();
                self.end_command_insert();
            }
            'u' => {
                let n = self.cmd_count();
                for _ in 0..n {
                    self.undo();
                }
                self.end_command(false);
            }
            '.' => {
                let n = self.count;
                self.end_command(false);
                self.repeat_last(n);
            }
            'v' => {
                self.enter_visual(false);
                self.end_command(false);
            }
            'V' => {
                self.enter_visual(true);
                self.end_command(false);
            }
            ':' => {
                self.mode = Mode::Command;
                self.cmdline.clear();
            }
            '/' => {
                self.mode = Mode::Search;
                self.cmdline.clear();
                self.last_search = Some((String::new(), true));
            }
            '?' => {
                self.mode = Mode::Search;
                self.cmdline.clear();
                self.last_search = Some((String::new(), false));
            }
            'n' => {
                self.search_repeat(false);
                self.end_command(false);
            }
            'N' => {
                self.search_repeat(true);
                self.end_command(false);
            }
            'm' => self.await_ = Await::Mark,
            '`' => self.await_ = Await::Goto { linewise: false },
            '\'' => self.await_ = Await::Goto { linewise: true },
            _ => self.end_command(false),
        }
    }

    /// Resolve a motion `key`, applying the pending operator if any.
    fn apply_motion(&mut self, key: char) {
        let cnt = self.motion_count();
        // `cw`/`cW` behave like `ce`/`cE` (vim's historical special case).
        let (mkey, arg) = if self.pending_op == Some('c') && (key == 'w' || key == 'W') {
            (if key == 'w' { 'e' } else { 'E' }, None)
        } else {
            (key, None)
        };
        let Some(mot) = self.compute_motion(mkey, arg, cnt) else {
            self.end_command(false);
            return;
        };
        if let Some(op) = self.pending_op {
            self.apply_operator(op, mot);
            let changed = op != 'y';
            if op == 'c' {
                // apply_operator already entered insert for `c`.
                self.end_command_insert();
            } else {
                self.end_command(changed);
            }
        } else {
            self.move_cursor(mot, key);
            self.end_command(false);
        }
    }

    fn resolve_await(&mut self, c: char) {
        let a = self.await_;
        self.await_ = Await::None;
        match a {
            Await::Find { forward, till } => {
                self.last_find = Some((forward, till, c));
                let key = match (forward, till) {
                    (true, false) => 'f',
                    (true, true) => 't',
                    (false, false) => 'F',
                    (false, true) => 'T',
                };
                let cnt = self.motion_count();
                if let Some(mot) = self.compute_motion(key, Some(c), cnt) {
                    if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
                        self.move_cursor(mot, key);
                        self.end_command(false);
                    } else if let Some(op) = self.pending_op {
                        self.apply_operator(op, mot);
                        if op == 'c' {
                            self.end_command_insert();
                        } else {
                            self.end_command(op != 'y');
                        }
                    } else {
                        self.move_cursor(mot, key);
                        self.end_command(false);
                    }
                } else {
                    self.end_command(false);
                }
            }
            Await::Replace => {
                self.push_undo();
                let n = self.cmd_count();
                let (l, col) = (self.cursor.line, self.cursor.col);
                if col + n <= self.len(l) {
                    for i in 0..n {
                        self.buf.delete_char(l, col + i);
                        let s = c.to_string();
                        self.buf.insert_str(l, col + i, &s);
                    }
                    self.cursor.col = col + n - 1;
                    self.touch();
                }
                self.end_command(true);
            }
            Await::Mark => {
                self.marks.insert(c, self.cursor);
                self.end_command(false);
            }
            Await::Goto { linewise } => {
                if let Some(&p) = self.marks.get(&c) {
                    self.cursor = self.clamp(p);
                    if linewise {
                        self.cursor.col = self.first_non_blank(self.cursor.line);
                    }
                }
                self.end_command(false);
            }
            Await::Register => {
                self.active_register = Some(c);
            }
            Await::G => {
                if c == 'g' {
                    let cnt = if self.explicit_count {
                        self.motion_count()
                    } else {
                        1
                    };
                    let l = cnt.saturating_sub(1).min(self.last_line());
                    let mot = Motion {
                        pos: Pos::new(l, self.first_non_blank(l)),
                        kind: MotionKind::Linewise,
                    };
                    if let Some(op) = self.pending_op {
                        self.apply_operator(op, mot);
                        self.end_command(op != 'y');
                    } else {
                        self.move_cursor(mot, 'g');
                        self.end_command(false);
                    }
                } else {
                    self.end_command(false);
                }
            }
            Await::None => {}
        }
    }

    fn repeat_find(&mut self, reverse: bool) {
        let Some((forward, till, ch)) = self.last_find else {
            self.end_command(false);
            return;
        };
        let forward = if reverse { !forward } else { forward };
        let key = match (forward, till) {
            (true, false) => 'f',
            (true, true) => 't',
            (false, false) => 'F',
            (false, true) => 'T',
        };
        let cnt = self.motion_count();
        if let Some(mot) = self.compute_motion(key, Some(ch), cnt) {
            if let Some(op) = self.pending_op {
                self.apply_operator(op, mot);
                self.end_command(op != 'y');
            } else {
                self.move_cursor(mot, key);
                self.end_command(false);
            }
        } else {
            self.end_command(false);
        }
    }

    // ---- Operator application ----

    pub(super) fn apply_operator(&mut self, op: char, mot: Motion) {
        match mot.kind {
            MotionKind::Linewise => {
                let a = self.cursor.line.min(mot.pos.line);
                let b = self.cursor.line.max(mot.pos.line);
                self.op_lines(op, a, b);
            }
            kind => {
                let (s, mut e) = order(self.cursor, mot.pos);
                if kind == MotionKind::Inclusive {
                    e.col += 1; // include the target char
                } else if e.line > s.line && e.col == 0 {
                    // Exclusive motion ending at column 0 → trim to prior EOL
                    // (vim's "dw at end of line" behavior; never eat the newline).
                    e = Pos::new(e.line - 1, self.len(e.line - 1));
                }
                if op == 'y' {
                    let text = self.buf.slice(s, e);
                    self.set_register(Register {
                        text,
                        linewise: false,
                    });
                    self.cursor = self.clamp(s);
                    return;
                }
                self.push_undo();
                let text = self.buf.delete_range(s, e);
                self.set_register(Register {
                    text,
                    linewise: false,
                });
                self.cursor = s;
                self.touch();
                if op == 'c' {
                    self.start_insert();
                } else {
                    self.clamp_cursor();
                }
            }
        }
    }

    /// Linewise operator over lines `a..=b`.
    pub(super) fn op_lines(&mut self, op: char, a: usize, b: usize) {
        let a = a.min(self.last_line());
        let b = b.min(self.last_line());
        let text = {
            let mut s = String::new();
            for i in a..=b {
                s.push_str(self.buf.line(i));
                if i < b {
                    s.push('\n');
                }
            }
            s
        };
        if op == 'y' {
            self.set_register(Register {
                text,
                linewise: true,
            });
            self.cursor.line = a;
            self.cursor.col = self.first_non_blank(a);
            return;
        }
        self.push_undo();
        self.set_register(Register {
            text,
            linewise: true,
        });
        if op == 'c' {
            // Change: clear the lines to a single empty line, then insert.
            for _ in a..=b {
                self.buf.remove_line(a);
            }
            self.buf.insert_line(a, String::new());
            self.cursor = Pos::new(a, 0);
            self.touch();
            self.start_insert();
        } else {
            for _ in a..=b {
                self.buf.remove_line(a);
            }
            self.cursor.line = a.min(self.last_line());
            self.cursor.col = self.first_non_blank(self.cursor.line);
            self.touch();
        }
    }

    fn paste(&mut self, after: bool) {
        let reg = self
            .active_register
            .and_then(|r| self.registers.get(&r).cloned())
            .unwrap_or_else(|| self.unnamed.clone());
        if reg.text.is_empty() {
            return;
        }
        self.push_undo();
        let n = self.cmd_count();
        if reg.linewise {
            let mut at = if after {
                self.cursor.line + 1
            } else {
                self.cursor.line
            };
            let first = at;
            for _ in 0..n {
                for line in reg.text.split('\n') {
                    self.buf.insert_line(at, line.to_string());
                    at += 1;
                }
            }
            self.cursor.line = first;
            self.cursor.col = self.first_non_blank(first);
        } else {
            let l = self.cursor.line;
            let mut col = if after {
                (self.cursor.col + 1).min(self.len(l))
            } else {
                self.cursor.col
            };
            for _ in 0..n {
                if reg.text.contains('\n') {
                    // Multi-line charwise paste: split the current line.
                    self.buf.split_line(l, col);
                    let parts: alloc::vec::Vec<&str> = reg.text.split('\n').collect();
                    self.buf.insert_str(l, col, parts[0]);
                    for (i, p) in parts[1..].iter().enumerate() {
                        self.buf.insert_line(l + 1 + i, p.to_string());
                    }
                    // simplistic cursor placement
                    self.cursor = Pos::new(l, col);
                    break;
                } else {
                    self.buf.insert_str(l, col, &reg.text);
                    col += reg.text.chars().count();
                }
            }
            self.cursor.col = col.saturating_sub(1).min(self.len(self.cursor.line));
        }
        self.touch();
    }

    fn set_register(&mut self, reg: Register) {
        if let Some(name) = self.active_register {
            self.registers.insert(name, reg.clone());
        }
        self.unnamed = reg;
    }

    // ---- Visual mode ----

    fn enter_visual(&mut self, linewise: bool) {
        self.visual_anchor = self.cursor;
        self.visual_linewise = linewise;
        self.mode = if linewise {
            Mode::VisualLine
        } else {
            Mode::Visual
        };
    }

    fn visual_char(&mut self, c: char) {
        match c {
            'o' => {
                core::mem::swap(&mut self.visual_anchor, &mut self.cursor);
                self.end_command(false);
            }
            'v' => {
                if self.mode == Mode::Visual {
                    self.mode = Mode::Normal;
                    self.clamp_cursor();
                } else {
                    self.mode = Mode::Visual;
                }
                self.end_command(false);
            }
            'V' => {
                if self.mode == Mode::VisualLine {
                    self.mode = Mode::Normal;
                    self.clamp_cursor();
                } else {
                    self.mode = Mode::VisualLine;
                }
                self.end_command(false);
            }
            'd' | 'x' | 'y' | 'c' | 's' => {
                let linewise = self.mode == Mode::VisualLine;
                let (a, b) = order(self.visual_anchor, self.cursor);
                let op = if c == 'x' {
                    'd'
                } else if c == 's' {
                    'c'
                } else {
                    c
                };
                self.mode = Mode::Normal;
                if linewise {
                    self.op_lines(op, a.line, b.line);
                    if op == 'c' {
                        self.end_command_insert();
                    } else {
                        self.end_command(op != 'y');
                    }
                } else {
                    let end = Pos::new(b.line, b.col + 1); // inclusive
                    if op == 'y' {
                        let text = self.buf.slice(a, end);
                        self.set_register(Register {
                            text,
                            linewise: false,
                        });
                        self.cursor = self.clamp(a);
                        self.end_command(false);
                    } else {
                        self.push_undo();
                        let text = self.buf.delete_range(a, end);
                        self.set_register(Register {
                            text,
                            linewise: false,
                        });
                        self.cursor = a;
                        self.touch();
                        if op == 'c' {
                            self.start_insert();
                            self.end_command_insert();
                        } else {
                            self.clamp_cursor();
                            self.end_command(true);
                        }
                    }
                }
            }
            _ => self.end_command(false),
        }
    }

    // ---- Insert mode ----

    fn start_insert(&mut self) {
        self.mode = Mode::Insert;
        self.in_insert_change = true;
    }
    /// After a command that entered insert, reset only the parse state (the
    /// change is not promoted to `.` until insert ends).
    fn end_command_insert(&mut self) {
        self.count = None;
        self.explicit_count = false;
        self.pending_op = None;
        self.op_count = None;
        self.await_ = Await::None;
        self.active_register = None;
    }

    pub(super) fn insert_char(&mut self, c: char) {
        self.buf.insert_char(self.cursor.line, self.cursor.col, c);
        self.cursor.col += 1;
        self.desired_col = self.cursor.col;
        self.touch();
    }

    pub(super) fn insert_special(&mut self, s: Special) {
        match s {
            Special::Esc => {
                self.mode = Mode::Normal;
                self.cursor.col = self.cursor.col.saturating_sub(1);
                self.clamp_cursor();
                self.in_insert_change = false;
                // Promote the whole insert session as one `.`-repeatable change.
                if !self.replaying {
                    self.last_change = core::mem::take(&mut self.cur_events);
                }
                self.cur_events.clear();
            }
            Special::Enter => {
                self.buf.split_line(self.cursor.line, self.cursor.col);
                self.cursor.line += 1;
                self.cursor.col = 0;
                self.touch();
            }
            Special::Backspace => {
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                    self.buf.delete_char(self.cursor.line, self.cursor.col);
                } else if self.cursor.line > 0 {
                    let prev = self.cursor.line - 1;
                    let at = self.len(prev);
                    self.buf.join_line(prev);
                    self.cursor = Pos::new(prev, at);
                }
                self.touch();
            }
            Special::Tab => self.insert_char('\t'),
            Special::Left => self.cursor.col = self.cursor.col.saturating_sub(1),
            Special::Right => {
                self.cursor.col = (self.cursor.col + 1).min(self.len(self.cursor.line))
            }
            Special::Up => {
                if self.cursor.line > 0 {
                    self.cursor.line -= 1;
                    self.cursor.col = self.cursor.col.min(self.len(self.cursor.line));
                }
            }
            Special::Down => {
                if self.cursor.line < self.last_line() {
                    self.cursor.line += 1;
                    self.cursor.col = self.cursor.col.min(self.len(self.cursor.line));
                }
            }
        }
    }

    pub(super) fn normal_special(&mut self, s: Special) {
        match s {
            Special::Esc => {
                if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
                    self.mode = Mode::Normal;
                    self.clamp_cursor();
                }
                self.end_command(false);
            }
            Special::Left | Special::Backspace => {
                if let Some(m) = self.compute_motion('h', None, self.cmd_count()) {
                    self.move_cursor(m, 'h');
                }
                self.end_command(false);
            }
            Special::Right => {
                if let Some(m) = self.compute_motion('l', None, self.cmd_count()) {
                    self.move_cursor(m, 'l');
                }
                self.end_command(false);
            }
            Special::Up => {
                if let Some(m) = self.compute_motion('k', None, self.cmd_count()) {
                    self.move_cursor(m, 'k');
                }
                self.end_command(false);
            }
            Special::Down | Special::Enter => {
                if let Some(m) = self.compute_motion('j', None, self.cmd_count()) {
                    self.move_cursor(m, 'j');
                }
                self.end_command(false);
            }
            Special::Tab => self.end_command(false),
        }
    }
}
