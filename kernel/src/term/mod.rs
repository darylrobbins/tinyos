//! Terminal widget + built-in command shell.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arch::timer;
use crate::drivers::input::keys;
use crate::gfx::font::Fonts;
use crate::gfx::surface::{argb, rgb, Surface};
use crate::{mem, VERSION};

pub const CELL_W: i32 = 9;
pub const CELL_H: i32 = 19;

const FG: u32 = rgb(228, 228, 236);
const ACCENT: u32 = rgb(120, 230, 190);
const DIM: u32 = rgb(150, 150, 168);
const ERR: u32 = rgb(255, 122, 110);

const PROMPT: &str = "daryl@tinyos ~ % ";
const SCROLLBACK: usize = 400;

pub struct Terminal {
    /// Wrap width in cells; the hosting card updates this from its rect.
    pub cols: usize,
    lines: VecDeque<(String, u32)>,
    input: String,
    cursor: usize,
    history: Vec<String>,
    hist_idx: Option<usize>,
}

impl Terminal {
    pub fn new() -> Self {
        let mut t = Self {
            cols: 80,
            lines: VecDeque::new(),
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            hist_idx: None,
        };
        t.out(format!("tinyOS {VERSION} - type 'help' to get started"), DIM);
        t
    }

    fn out(&mut self, s: String, color: u32) {
        // Wrap to the grid width.
        let chars: Vec<char> = s.chars().collect();
        if chars.is_empty() {
            self.push_line(String::new(), color);
        }
        for chunk in chars.chunks(self.cols.max(1)) {
            self.push_line(chunk.iter().collect(), color);
        }
    }

    fn push_line(&mut self, s: String, color: u32) {
        if self.lines.len() >= SCROLLBACK {
            self.lines.pop_front();
        }
        self.lines.push_back((s, color));
    }

    pub fn on_char(&mut self, c: char) {
        if c == '\n' {
            self.execute();
        } else {
            self.input.insert(self.byte_cursor(), c);
            self.cursor += 1;
            self.hist_idx = None;
        }
    }

    pub fn on_key(&mut self, code: u16) {
        match code {
            keys::BACKSPACE => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.input.remove(self.byte_cursor());
                }
            }
            keys::LEFT => self.cursor = self.cursor.saturating_sub(1),
            keys::RIGHT => self.cursor = (self.cursor + 1).min(self.input.chars().count()),
            keys::UP => self.history_nav(true),
            keys::DOWN => self.history_nav(false),
            _ => {}
        }
    }

    fn byte_cursor(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    fn history_nav(&mut self, up: bool) {
        if self.history.is_empty() {
            return;
        }
        let idx = match (self.hist_idx, up) {
            (None, true) => Some(self.history.len() - 1),
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), false) => None,
        };
        self.hist_idx = idx;
        self.input = idx.map(|i| self.history[i].clone()).unwrap_or_default();
        self.cursor = self.input.chars().count();
    }

    fn execute(&mut self) {
        let cmd = self.input.trim().to_string();
        let echo = format!("{PROMPT}{}", self.input);
        self.out(echo, DIM);
        self.input.clear();
        self.cursor = 0;
        self.hist_idx = None;

        if cmd.is_empty() {
            return;
        }
        self.history.push(cmd.clone());

        let (name, rest) = cmd.split_once(' ').unwrap_or((cmd.as_str(), ""));
        match name {
            "help" => {
                self.out("commands:".to_string(), FG);
                for (c, d) in [
                    ("help", "this list"),
                    ("echo <text>", "print text"),
                    ("clear", "clear the screen"),
                    ("sysinfo", "hardware and kernel info"),
                    ("memstat", "heap usage"),
                    ("uptime", "time since boot"),
                    ("date", "current date and time"),
                    ("about", "about tinyOS"),
                ] {
                    self.out(format!("  {c:<14} {d}"), FG);
                }
            }
            "echo" => self.out(rest.to_string(), FG),
            "clear" => self.lines.clear(),
            "sysinfo" => {
                let (used, free) = mem::stats();
                let lines = [
                    format!("tinyOS {VERSION}"),
                    format!("arch:      {} ({})", crate::arch::NAME, crate::arch::boot_privilege()),
                    format!("machine:   {}", crate::arch::MACHINE),
                    format!("display:   {}x{} @ 32bpp", crate::fb_size().0, crate::fb_size().1),
                    format!("heap:      {} MiB used / {} MiB free", used >> 20, free >> 20),
                    format!("uptime:    {}", fmt_uptime()),
                ];
                for l in lines {
                    self.out(l, FG);
                }
            }
            "memstat" => {
                let (used, free) = mem::stats();
                self.out(
                    format!("heap: {} KiB used, {} KiB free", used >> 10, free >> 10),
                    FG,
                );
            }
            "uptime" => self.out(fmt_uptime(), FG),
            "date" => self.out(fmt_date(), FG),
            "about" => {
                self.out("tinyOS - a tiny operating system written in Rust".to_string(), ACCENT);
                self.out("UEFI boot, software-composited GUI, no interrupts,".to_string(), FG);
                self.out("no processes, no problems.".to_string(), FG);
            }
            "sudo" => self.out(
                "daryl is not in the sudoers file. This incident will be reported.".to_string(),
                ERR,
            ),
            _ => self.out(format!("command not found: {name}"), ERR),
        }
    }

    pub fn draw(
        &self,
        surface: &mut Surface,
        fonts: &mut Fonts,
        ox: i32,
        oy: i32,
        rows: usize,
        now_ms: u64,
    ) {
        // Visible slice: last rows-1 scrollback lines, prompt on the next row.
        let visible = rows.saturating_sub(1).max(1);
        let start = self.lines.len().saturating_sub(visible);
        let mut row = 0;
        for (text, color) in self.lines.iter().skip(start) {
            fonts
                .mono
                .draw(surface, text, 15.0, ox, oy + row as i32 * CELL_H, *color);
            row += 1;
        }

        // Prompt line with block cursor.
        let y = oy + row as i32 * CELL_H;
        fonts.mono.draw(surface, PROMPT, 15.0, ox, y, ACCENT);
        let px = ox + PROMPT.len() as i32 * CELL_W;
        fonts.mono.draw(surface, &self.input, 15.0, px, y, FG);

        if now_ms / 530 % 2 == 0 {
            let cx = px + self.cursor as i32 * CELL_W;
            surface.fill_rect(cx, y + 1, CELL_W, CELL_H - 2, argb(210, 228, 228, 236));
        }
    }
}

fn fmt_uptime() -> String {
    let s = timer::uptime_ms() / 1000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn fmt_date() -> String {
    // Boot pretends it is Fri Jul 17 2026, 9:41 am.
    let total_s = 9 * 3600 + 41 * 60 + timer::uptime_ms() / 1000;
    format!(
        "Fri Jul 17 {:02}:{:02}:{:02} 2026",
        total_s / 3600 % 24,
        total_s / 60 % 60,
        total_s % 60
    )
}
