//! Ex command line (`:`) and search (`/`, `?`) handling.

use alloc::string::ToString;
use alloc::vec::Vec;

use super::{Editor, Effect, Mode};
use crate::buffer::Pos;

impl Editor {
    pub(super) fn cmdline_char(&mut self, c: char) {
        self.cmdline.push(c);
    }

    pub(super) fn cmdline_special(&mut self, s: super::Special) {
        use super::Special::*;
        match s {
            Esc => {
                self.mode = Mode::Normal;
                self.cmdline.clear();
                self.clamp_cursor();
            }
            Enter => {
                let line = core::mem::take(&mut self.cmdline);
                let was_search = self.mode == Mode::Search;
                self.mode = Mode::Normal;
                if was_search {
                    let forward = self.last_search.as_ref().map(|s| s.1).unwrap_or(true);
                    self.run_search(&line, forward);
                } else {
                    self.execute_ex(&line);
                }
            }
            Backspace => {
                if self.cmdline.pop().is_none() {
                    self.mode = Mode::Normal;
                }
            }
            _ => {}
        }
    }

    fn execute_ex(&mut self, cmd: &str) {
        let cmd = cmd.trim();
        self.status.clear();
        if cmd.is_empty() {
            return;
        }
        // `:N` — jump to line N.
        if cmd.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = cmd.parse::<usize>() {
                let l = n.saturating_sub(1).min(self.last_line());
                self.cursor = Pos::new(l, self.first_non_blank(l));
            }
            return;
        }
        // Substitution: `s/…`, `%s/…`, or `.s/…`.
        if cmd.starts_with("s") && cmd.len() > 1 && !cmd.starts_with("se") {
            self.substitute(&cmd[1..], false);
            return;
        }
        if let Some(rest) = cmd.strip_prefix("%s") {
            self.substitute(rest, true);
            return;
        }

        let (name, arg) = match cmd.split_once(' ') {
            Some((n, a)) => (n, a.trim()),
            None => (cmd, ""),
        };
        let path = if arg.is_empty() {
            None
        } else {
            Some(arg.to_string())
        };
        match name {
            "w" | "w!" => self.effects.push(Effect::Save(path)),
            "q" => {
                if self.dirty {
                    self.status = "E37: No write since last change (add ! to override)".to_string();
                } else {
                    self.effects.push(Effect::Quit);
                }
            }
            "q!" => self.effects.push(Effect::ForceQuit),
            "wq" | "x" | "wq!" | "x!" => self.effects.push(Effect::SaveQuit(path)),
            "qa" | "qall" => {
                if self.dirty {
                    self.status = "E37: No write since last change (add ! to override)".to_string();
                } else {
                    self.effects.push(Effect::Quit);
                }
            }
            "qa!" | "qall!" => self.effects.push(Effect::ForceQuit),
            _ => {
                self.status = alloc::format!("E492: Not an editor command: {name}");
            }
        }
    }

    /// Handle the body of `:s<delim>pat<delim>rep<delim>flags` (delim is the
    /// first char). `whole` = apply to every line (`%s`).
    fn substitute(&mut self, spec: &str, whole: bool) {
        let mut chars = spec.chars();
        let Some(delim) = chars.next() else {
            self.status = "E486: pattern required".to_string();
            return;
        };
        if delim.is_alphanumeric() {
            self.status = "E486: pattern required".to_string();
            return;
        }
        let parts: Vec<&str> = spec[delim.len_utf8()..].split(delim).collect();
        let pat = parts.first().copied().unwrap_or("");
        let rep = parts.get(1).copied().unwrap_or("");
        let flags = parts.get(2).copied().unwrap_or("");
        if pat.is_empty() {
            self.status = "E486: pattern required".to_string();
            return;
        }
        let global = flags.contains('g');
        let range: Vec<usize> = if whole {
            (0..self.buf.line_count()).collect()
        } else {
            alloc::vec![self.cursor.line]
        };

        let mut changed = false;
        let mut count = 0usize;
        let mut last_line = self.cursor.line;
        let mut snapshotted = false;
        for l in range {
            let src = self.buf.line(l).to_string();
            if !src.contains(pat) {
                continue;
            }
            let new = if global {
                src.replace(pat, rep)
            } else {
                src.replacen(pat, rep, 1)
            };
            if new != src {
                if !snapshotted {
                    self.push_undo();
                    snapshotted = true;
                }
                count += src.matches(pat).count().min(if global { usize::MAX } else { 1 });
                self.buf.set_line(l, new);
                changed = true;
                last_line = l;
            }
        }
        if changed {
            self.cursor = Pos::new(last_line, 0);
            self.clamp_cursor();
            self.touch();
        } else {
            self.status = alloc::format!("E486: pattern not found: {pat}");
        }
        let _ = count;
    }

    // ---- Search ----

    fn run_search(&mut self, pat: &str, forward: bool) {
        let pat = if pat.is_empty() {
            // Reuse the previous pattern on a bare `/`.
            match &self.last_search {
                Some((p, _)) if !p.is_empty() => p.clone(),
                _ => {
                    self.status = "E35: No previous regular expression".to_string();
                    return;
                }
            }
        } else {
            pat.to_string()
        };
        self.last_search = Some((pat.clone(), forward));
        match self.find_match(&pat, forward, self.cursor) {
            Some(p) => {
                self.cursor = self.clamp(p);
                self.status.clear();
            }
            None => self.status = alloc::format!("E486: pattern not found: {pat}"),
        }
    }

    pub(super) fn search_repeat(&mut self, reverse: bool) {
        let Some((pat, forward)) = self.last_search.clone() else {
            return;
        };
        if pat.is_empty() {
            return;
        }
        let dir = if reverse { !forward } else { forward };
        if let Some(p) = self.find_match(&pat, dir, self.cursor) {
            self.cursor = self.clamp(p);
            self.status.clear();
        } else {
            self.status = alloc::format!("E486: pattern not found: {pat}");
        }
    }

    /// All match start positions in document order.
    fn matches(&self, pat: &str) -> Vec<Pos> {
        let mut out = Vec::new();
        for (l, line) in self.buf.lines().iter().enumerate() {
            for (b, _) in line.match_indices(pat) {
                let col = line[..b].chars().count();
                out.push(Pos::new(l, col));
            }
        }
        out
    }

    fn find_match(&self, pat: &str, forward: bool, from: Pos) -> Option<Pos> {
        let all = self.matches(pat);
        if all.is_empty() {
            return None;
        }
        if forward {
            all.iter()
                .find(|&&p| p > from)
                .copied()
                .or_else(|| all.first().copied())
        } else {
            all.iter()
                .rev()
                .find(|&&p| p < from)
                .copied()
                .or_else(|| all.last().copied())
        }
    }
}
