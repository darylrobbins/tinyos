//! The text buffer: a list of lines with char-indexed columns.
//!
//! Columns are **character** indices (not bytes), so all edits are UTF-8 safe.
//! The buffer always holds at least one line (an empty document is `[""]`),
//! matching vi, where you always have a current line.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// A char-indexed position in the buffer. `line` and `col` are both 0-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    lines: Vec<String>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    /// An empty buffer holding a single empty line.
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }

    /// Load text, splitting on `\n`. A trailing newline is *not* treated as an
    /// extra empty line unless the text is empty. `"a\nb"` and `"a\nb\n"` both
    /// yield two lines `["a", "b"]`; the terminating newline is re-added on save.
    pub fn from_str(text: &str) -> Self {
        if text.is_empty() {
            return Self::new();
        }
        let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
        // A final `\n` produces a trailing empty element we drop so the document
        // round-trips (save appends the newline back).
        if lines.len() > 1 && lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        Self { lines }
    }

    /// Serialize to text with a trailing newline (Unix convention).
    pub fn to_string(&self) -> String {
        let mut out = self.lines.join("\n");
        out.push('\n');
        out
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The text of line `i`, or `""` if out of range.
    pub fn line(&self, i: usize) -> &str {
        self.lines.get(i).map(String::as_str).unwrap_or("")
    }

    /// Number of characters on line `i`.
    pub fn line_len(&self, i: usize) -> usize {
        self.lines.get(i).map(|l| l.chars().count()).unwrap_or(0)
    }

    /// Whether the buffer is a single empty line.
    pub fn is_empty_doc(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Byte offset of char column `col` within line `i` (for slicing).
    pub fn byte_index(&self, i: usize, col: usize) -> usize {
        let line = self.line(i);
        line.char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(line.len())
    }

    /// Insert a character at `(line, col)`. Clamps `col` to the line length.
    pub fn insert_char(&mut self, line: usize, col: usize, c: char) {
        if line >= self.lines.len() {
            return;
        }
        let b = self.byte_index(line, col);
        self.lines[line].insert(b, c);
    }

    /// Insert a whole string at `(line, col)` (no embedded newlines expected).
    pub fn insert_str(&mut self, line: usize, col: usize, s: &str) {
        if line >= self.lines.len() {
            return;
        }
        let b = self.byte_index(line, col);
        self.lines[line].insert_str(b, s);
    }

    /// Delete the character at `(line, col)`, returning it. No-op past EOL.
    pub fn delete_char(&mut self, line: usize, col: usize) -> Option<char> {
        if line >= self.lines.len() || col >= self.line_len(line) {
            return None;
        }
        let b = self.byte_index(line, col);
        Some(self.lines[line].remove(b))
    }

    /// Split `line` at `col`: text at/after `col` becomes a new following line.
    pub fn split_line(&mut self, line: usize, col: usize) {
        if line >= self.lines.len() {
            return;
        }
        let b = self.byte_index(line, col);
        let tail = self.lines[line].split_off(b);
        self.lines.insert(line + 1, tail);
    }

    /// Append `line + 1` onto the end of `line` (raw join, no separator).
    /// Returns the char column where the join happened, or `None` if there is
    /// no following line.
    pub fn join_line(&mut self, line: usize) -> Option<usize> {
        if line + 1 >= self.lines.len() {
            return None;
        }
        let at = self.line_len(line);
        let next = self.lines.remove(line + 1);
        self.lines[line].push_str(&next);
        Some(at)
    }

    pub fn insert_line(&mut self, idx: usize, text: String) {
        let idx = idx.min(self.lines.len());
        self.lines.insert(idx, text);
    }

    /// Remove line `idx`, returning its text. The buffer never becomes empty:
    /// removing the last remaining line leaves a single empty line.
    pub fn remove_line(&mut self, idx: usize) -> String {
        if idx >= self.lines.len() {
            return String::new();
        }
        let removed = self.lines.remove(idx);
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        removed
    }

    /// Replace the entire contents of line `idx`.
    pub fn set_line(&mut self, idx: usize, text: String) {
        if idx < self.lines.len() {
            self.lines[idx] = text;
        }
    }

    /// Extract the charwise text between `start` and `end` (end exclusive).
    /// Multi-line ranges include the intervening `\n`s.
    pub fn slice(&self, start: Pos, end: Pos) -> String {
        let (start, end) = order(start, end);
        if start.line == end.line {
            let l = self.line(start.line);
            let b0 = self.byte_index(start.line, start.col);
            let b1 = self.byte_index(start.line, end.col);
            return l[b0..b1].into();
        }
        let mut out = String::new();
        let first = self.line(start.line);
        out.push_str(&first[self.byte_index(start.line, start.col)..]);
        out.push('\n');
        for i in (start.line + 1)..end.line {
            out.push_str(self.line(i));
            out.push('\n');
        }
        let last = self.line(end.line);
        out.push_str(&last[..self.byte_index(end.line, end.col)]);
        out
    }

    /// Delete the charwise range `[start, end)` and return the removed text.
    /// The cursor should land at `start`.
    pub fn delete_range(&mut self, start: Pos, end: Pos) -> String {
        let (start, end) = order(start, end);
        let removed = self.slice(start, end);
        if start.line == end.line {
            let b0 = self.byte_index(start.line, start.col);
            let b1 = self.byte_index(start.line, end.col);
            self.lines[start.line].replace_range(b0..b1, "");
        } else {
            let b0 = self.byte_index(start.line, start.col);
            let tail_start = self.byte_index(end.line, end.col);
            let tail: String = self.lines[end.line][tail_start..].into();
            self.lines[start.line].truncate(b0);
            self.lines[start.line].push_str(&tail);
            // Drain the fully/partly consumed lines below `start.line`.
            self.lines.drain((start.line + 1)..=end.line);
        }
        removed
    }

    /// Access to the raw lines (rendering).
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

/// Return `(a, b)` ordered so `a <= b`.
pub fn order(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn empty_doc_is_one_empty_line() {
        let b = Buffer::new();
        assert_eq!(b.line_count(), 1);
        assert!(b.is_empty_doc());
        assert_eq!(b.to_string(), "\n");
    }

    #[test]
    fn from_str_round_trips() {
        assert_eq!(Buffer::from_str("a\nb").to_string(), "a\nb\n");
        assert_eq!(Buffer::from_str("a\nb\n").to_string(), "a\nb\n");
        assert_eq!(Buffer::from_str("").to_string(), "\n");
        assert_eq!(Buffer::from_str("x").line_count(), 1);
        assert_eq!(Buffer::from_str("a\nb\n").line_count(), 2);
    }

    #[test]
    fn insert_and_delete_char_utf8() {
        let mut b = Buffer::from_str("héllo");
        b.insert_char(0, 1, 'X');
        assert_eq!(b.line(0), "hXéllo");
        assert_eq!(b.delete_char(0, 1), Some('X'));
        assert_eq!(b.line(0), "héllo");
        // delete past EOL is a no-op
        assert_eq!(b.delete_char(0, 99), None);
    }

    #[test]
    fn split_and_join() {
        let mut b = Buffer::from_str("hello world");
        b.split_line(0, 5);
        assert_eq!(b.line(0), "hello");
        assert_eq!(b.line(1), " world");
        assert_eq!(b.join_line(0), Some(5));
        assert_eq!(b.line(0), "hello world");
        assert_eq!(b.join_line(0), None);
    }

    #[test]
    fn remove_line_keeps_one() {
        let mut b = Buffer::from_str("only");
        assert_eq!(b.remove_line(0), "only");
        assert!(b.is_empty_doc());
    }

    #[test]
    fn slice_single_and_multi_line() {
        let b = Buffer::from_str("hello\nbrave\nworld");
        assert_eq!(b.slice(Pos::new(0, 0), Pos::new(0, 5)), "hello");
        assert_eq!(
            b.slice(Pos::new(0, 3), Pos::new(2, 2)),
            "lo\nbrave\nwo".to_string()
        );
        // ordering is normalized
        assert_eq!(b.slice(Pos::new(0, 5), Pos::new(0, 0)), "hello");
    }

    #[test]
    fn delete_range_single_line() {
        let mut b = Buffer::from_str("hello world");
        let got = b.delete_range(Pos::new(0, 0), Pos::new(0, 6));
        assert_eq!(got, "hello ");
        assert_eq!(b.line(0), "world");
    }

    #[test]
    fn delete_range_multi_line_joins() {
        let mut b = Buffer::from_str("hello\nbrave\nworld");
        let got = b.delete_range(Pos::new(0, 3), Pos::new(2, 2));
        assert_eq!(got, "lo\nbrave\nwo");
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line(0), "helrld");
    }
}
