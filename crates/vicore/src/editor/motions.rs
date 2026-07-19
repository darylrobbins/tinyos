//! Motion computation shared by cursor movement and operators.

use super::{class, Class, Editor};
use crate::buffer::Pos;

/// How a motion's target combines with an operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MotionKind {
    /// Range is `[from, to)` — the target column is excluded (`w`, `h`, `0`).
    Exclusive,
    /// Range includes the target char (`e`, `$`, `f`).
    Inclusive,
    /// Whole lines (`j`, `k`, `gg`, `G`).
    Linewise,
}

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub pos: Pos,
    pub kind: MotionKind,
}

impl Editor {
    pub(super) fn ch(&self, l: usize, c: usize) -> Option<char> {
        self.buf.line(l).chars().nth(c)
    }
    pub(super) fn len(&self, l: usize) -> usize {
        self.buf.line_len(l)
    }
    pub(super) fn first_non_blank(&self, l: usize) -> usize {
        let line = self.buf.line(l);
        line.chars()
            .position(|c| !c.is_whitespace())
            .unwrap_or(0)
    }
    /// Compute a motion from the current cursor. `arg` is the target char for
    /// `f F t T`. Returns `None` for an unknown/blocked motion.
    pub(super) fn compute_motion(
        &self,
        key: char,
        arg: Option<char>,
        count: usize,
    ) -> Option<Motion> {
        let cur = self.cursor;
        let ex = MotionKind::Exclusive;
        let inc = MotionKind::Inclusive;
        let line = MotionKind::Linewise;
        let m = |pos, kind| Some(Motion { pos, kind });
        match key {
            'h' => {
                let c = cur.col.saturating_sub(count);
                m(Pos::new(cur.line, c), ex)
            }
            'l' | ' ' => {
                let maxc = self.len(cur.line);
                let c = (cur.col + count).min(maxc.saturating_sub(0));
                m(Pos::new(cur.line, c.min(maxc)), ex)
            }
            'j' => {
                let l = (cur.line + count).min(self.last_line());
                m(Pos::new(l, cur.col), line)
            }
            'k' => {
                let l = cur.line.saturating_sub(count);
                m(Pos::new(l, cur.col), line)
            }
            '0' => m(Pos::new(cur.line, 0), ex),
            '^' => m(Pos::new(cur.line, self.first_non_blank(cur.line)), ex),
            '$' => {
                let l = (cur.line + count - 1).min(self.last_line());
                let c = self.len(l).saturating_sub(1);
                m(Pos::new(l, c), inc)
            }
            '|' => m(Pos::new(cur.line, count.saturating_sub(1)), ex),
            'G' => {
                // With an explicit count, `{N}G` goes to line N (1-based).
                let l = if self.explicit_count {
                    (count.saturating_sub(1)).min(self.last_line())
                } else {
                    self.last_line()
                };
                m(Pos::new(l, self.first_non_blank(l)), line)
            }
            'w' => m(self.word_fwd(cur, false, count), ex),
            'W' => m(self.word_fwd(cur, true, count), ex),
            'b' => m(self.word_back(cur, false, count), ex),
            'B' => m(self.word_back(cur, true, count), ex),
            'e' => m(self.word_end(cur, false, count), inc),
            'E' => m(self.word_end(cur, true, count), inc),
            'f' | 'F' | 't' | 'T' => {
                let target = arg?;
                let forward = key == 'f' || key == 't';
                let till = key == 't' || key == 'T';
                let pos = self.find_char(cur, target, forward, till, count)?;
                m(pos, if forward { inc } else { ex })
            }
            'H' => {
                let l = (self.view_top + count.saturating_sub(1)).min(self.last_line());
                m(Pos::new(l, self.first_non_blank(l)), line)
            }
            'L' => {
                let bottom = (self.view_top + self.view_rows.saturating_sub(1)).min(self.last_line());
                let l = bottom.saturating_sub(count.saturating_sub(1));
                m(Pos::new(l, self.first_non_blank(l)), line)
            }
            'M' => {
                let bottom = (self.view_top + self.view_rows.saturating_sub(1)).min(self.last_line());
                let l = (self.view_top + bottom) / 2;
                m(Pos::new(l, self.first_non_blank(l)), line)
            }
            _ => None,
        }
    }

    fn word_fwd(&self, from: Pos, big: bool, count: usize) -> Pos {
        let (mut l, mut c) = (from.line, from.col);
        for _ in 0..count {
            let start_cls = self.class_at(l, c, big);
            // Skip the rest of the current run (if on a non-blank).
            if start_cls != Some(Class::Blank) && start_cls.is_some() {
                while self.class_at(l, c, big) == start_cls {
                    c += 1;
                }
            }
            // Skip blanks (and line breaks) to the next word start.
            loop {
                if c >= self.len(l) {
                    if l < self.last_line() {
                        l += 1;
                        c = 0;
                        if self.len(l) == 0 {
                            break; // empty line counts as a word
                        }
                    } else {
                        c = self.len(l);
                        break;
                    }
                } else if self.class_at(l, c, big) == Some(Class::Blank) {
                    c += 1;
                } else {
                    break;
                }
            }
        }
        Pos::new(l, c)
    }

    fn word_back(&self, from: Pos, big: bool, count: usize) -> Pos {
        let (mut l, mut c) = (from.line, from.col);
        for _ in 0..count {
            // Step back one position (across line breaks).
            if c == 0 {
                if l == 0 {
                    break;
                }
                l -= 1;
                c = self.len(l);
            } else {
                c -= 1;
            }
            // Skip blanks backward.
            loop {
                if c >= self.len(l) || self.class_at(l, c, big) == Some(Class::Blank) {
                    if c == 0 {
                        if l == 0 || self.len(l) == 0 {
                            break;
                        }
                        l -= 1;
                        c = self.len(l).saturating_sub(1);
                    } else {
                        c -= 1;
                    }
                } else {
                    break;
                }
            }
            // Move to the start of this run.
            let cls = self.class_at(l, c, big);
            while c > 0 && self.class_at(l, c - 1, big) == cls {
                c -= 1;
            }
        }
        Pos::new(l, c)
    }

    fn word_end(&self, from: Pos, big: bool, count: usize) -> Pos {
        let (mut l, mut c) = (from.line, from.col);
        for _ in 0..count {
            // Step forward one.
            if c + 1 >= self.len(l) {
                if l < self.last_line() {
                    l += 1;
                    c = 0;
                } else {
                    c = self.len(l).saturating_sub(1);
                    break;
                }
            } else {
                c += 1;
            }
            // Skip blanks forward.
            loop {
                if c >= self.len(l) {
                    if l < self.last_line() {
                        l += 1;
                        c = 0;
                    } else {
                        break;
                    }
                } else if self.class_at(l, c, big) == Some(Class::Blank) {
                    c += 1;
                } else {
                    break;
                }
            }
            // Advance to the end of this run.
            let cls = self.class_at(l, c, big);
            while c + 1 < self.len(l) && self.class_at(l, c + 1, big) == cls {
                c += 1;
            }
        }
        Pos::new(l, c)
    }

    fn find_char(
        &self,
        from: Pos,
        target: char,
        forward: bool,
        till: bool,
        count: usize,
    ) -> Option<Pos> {
        let l = from.line;
        let mut c = from.col;
        let mut hits = 0;
        if forward {
            let mut i = c + 1;
            // For `t`, if already just before the target from a repeat, skip it.
            while i < self.len(l) {
                if self.ch(l, i) == Some(target) {
                    hits += 1;
                    if hits == count {
                        return Some(Pos::new(l, if till { i - 1 } else { i }));
                    }
                }
                i += 1;
            }
            None
        } else {
            if c == 0 {
                return None;
            }
            loop {
                c -= 1;
                if self.ch(l, c) == Some(target) {
                    hits += 1;
                    if hits == count {
                        return Some(Pos::new(l, if till { c + 1 } else { c }));
                    }
                }
                if c == 0 {
                    break;
                }
            }
            None
        }
    }

    fn class_at(&self, l: usize, c: usize, big: bool) -> Option<Class> {
        let ch = self.ch(l, c)?;
        Some(if big {
            if ch.is_whitespace() {
                Class::Blank
            } else {
                Class::Word
            }
        } else {
            class(ch)
        })
    }

    /// Apply a plain cursor motion (no operator). Updates the sticky column.
    pub(super) fn move_cursor(&mut self, mot: Motion, key: char) {
        match key {
            'j' | 'k' => {
                // Preserve the desired column across vertical moves.
                let want = if self.desired_col == usize::MAX {
                    usize::MAX
                } else {
                    self.desired_col
                };
                self.cursor.line = mot.pos.line;
                let maxc = self.max_col(self.cursor.line);
                self.cursor.col = if want == usize::MAX { maxc } else { want.min(maxc) };
            }
            '$' => {
                self.cursor = mot.pos;
                self.cursor = self.clamp(self.cursor);
                self.desired_col = usize::MAX;
            }
            _ => {
                self.cursor = self.clamp(mot.pos);
                self.desired_col = self.cursor.col;
            }
        }
    }

    pub(super) fn page(&mut self, delta: isize) {
        let new = self.cursor.line as isize + delta;
        let new = new.clamp(0, self.last_line() as isize) as usize;
        self.cursor.line = new;
        self.cursor = self.clamp(self.cursor);
        // Keep the viewport following.
        self.view_top = self
            .view_top
            .saturating_add_signed(delta)
            .min(self.last_line());
    }
}
