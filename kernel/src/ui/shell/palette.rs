//! Command palette: Ctrl+K launcher/switcher with inline answers.

use alloc::format;
use alloc::string::{String, ToString};

use crate::drivers::input::keys;
use crate::gfx::font::Fonts;
use crate::gfx::surface::{argb, Surface};

use super::tokens::*;

pub const BAR_W: i32 = 620;
pub const BAR_H: i32 = 52;

pub enum Action {
    None,
    Dismiss,
    Open(&'static str),
    CloseFocused,
    Help,
    Calc(String),
    Timer(u64),
    Unknown(String),
}

pub struct Palette {
    pub open: bool,
    pub input: String,
    cursor: usize,
    pub hint: Option<String>,
}

impl Palette {
    pub fn new() -> Self {
        Self {
            open: false,
            input: String::new(),
            cursor: 0,
            hint: None,
        }
    }

    pub fn summon(&mut self) {
        self.open = true;
        self.input.clear();
        self.cursor = 0;
        self.hint = None;
    }

    pub fn dismiss(&mut self) {
        self.open = false;
        self.hint = None;
    }

    fn byte_cursor(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    pub fn on_char(&mut self, c: char) {
        if c != '\n' {
            self.input.insert(self.byte_cursor(), c);
            self.cursor += 1;
            self.hint = None;
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
            _ => {}
        }
    }

    /// Parse and clear the input; the Deck acts on the result.
    pub fn submit(&mut self) -> Action {
        let cmd = self.input.trim().to_string();
        self.input.clear();
        self.cursor = 0;

        if cmd.is_empty() {
            return Action::None;
        }
        if let Some(expr) = cmd.strip_prefix('=') {
            return Action::Calc(expr.to_string());
        }
        if let Some(arg) = cmd.strip_prefix("timer") {
            let arg = arg.trim();
            let (num, unit) = arg.split_at(arg.len().saturating_sub(1));
            let mult = match unit {
                "m" => 60,
                "s" => 1,
                _ => 0,
            };
            if mult > 0 {
                if let Ok(n) = num.parse::<u64>() {
                    return Action::Timer(n * mult);
                }
            }
            self.hint = Some("usage: timer 5m or timer 30s".to_string());
            return Action::None;
        }
        match cmd.as_str() {
            "terminal" | "notes" | "monitor" | "clock" => Action::Open(match cmd.as_str() {
                "terminal" => "terminal",
                "notes" => "notes",
                "monitor" => "monitor",
                _ => "clock",
            }),
            "close" => Action::CloseFocused,
            "help" => Action::Help,
            _ => {
                self.hint = Some(format!("no such command: {cmd} \u{2014} try help"));
                Action::Unknown(cmd)
            }
        }
    }

    pub fn draw(&self, s: &mut Surface, fonts: &mut Fonts, screen_w: i32, now_ms: u64) {
        if !self.open {
            return;
        }
        let x = (screen_w - BAR_W) / 2;
        let y = STATUS_H + 96;
        // The panel grows to hold the hint line inside it.
        let panel_h = if self.hint.is_some() { BAR_H + 30 } else { BAR_H };

        // Soft shadow + surface.
        for i in 0..4 {
            let spread = 4 * (i + 1);
            s.fill_rounded_rect(
                x - spread / 2,
                y - spread / 2 + 4,
                BAR_W + spread,
                panel_h + spread,
                RADIUS + spread / 2,
                argb(10, 0, 0, 0),
            );
        }
        s.fill_rounded_rect(x, y, BAR_W, panel_h, RADIUS, SURFACE_HI);
        s.fill_rect(x + RADIUS, y, BAR_W - 2 * RADIUS, 1, BORDER);
        s.fill_rect(x + RADIUS, y + panel_h - 1, BAR_W - 2 * RADIUS, 1, BORDER);
        s.fill_rect(x, y + RADIUS, 1, panel_h - 2 * RADIUS, BORDER);
        s.fill_rect(x + BAR_W - 1, y + RADIUS, 1, panel_h - 2 * RADIUS, BORDER);

        fonts.mono.draw(s, ">", 17.0, x + 18, y + 16, TEXT_DIM);
        fonts.mono.draw(s, &self.input, 17.0, x + 38, y + 16, TEXT);

        if now_ms / 530 % 2 == 0 {
            let cx = x + 38 + self.cursor as i32 * 10;
            s.fill_rect(cx, y + 13, 2, BAR_H - 26, ACCENT);
        }

        if let Some(hint) = &self.hint {
            s.fill_rect(x + 16, y + BAR_H, BAR_W - 32, 1, BORDER);
            fonts
                .ui_medium
                .draw(s, hint, 13.0, x + 18, y + BAR_H + 7, TEXT_DIM);
        }
    }
}

