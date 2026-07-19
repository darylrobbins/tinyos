//! The Meridian launcher: Ctrl+K glass sheet — ask-anything input,
//! suggested actions, app grid, and session footer.

use alloc::format;
use alloc::string::{String, ToString};

use crate::drivers::input::keys;
use crate::gfx::font::Fonts;
use crate::gfx::surface::{lerp, Surface};

use super::dock;
use super::tokens::*;

pub const SUGGESTIONS: [(&str, &str); 3] = [
    ("Open the system monitor", "monitor"),
    ("Start a 5 minute timer", "timer 5m"),
    ("Calculate  = 240/8", "= 240/8"),
];

pub enum Action {
    None,
    Dismiss,
    Open(&'static str),
    CloseFocused,
    Help,
    Calc(String),
    Timer(u64),
    Lock,
    Unknown(String),
}

pub enum LauncherHit {
    Suggestion(usize),
    App(&'static str),
    Lock,
    Inside,
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

    /// Submit an arbitrary command string (used by suggestion rows).
    pub fn submit_text(&mut self, cmd: &str) -> Action {
        self.input = String::from(cmd);
        self.cursor = self.input.chars().count();
        self.submit()
    }

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
            "terminal" | "notes" | "monitor" | "clock" | "solitaire" | "pixels" => {
                Action::Open(match cmd.as_str() {
                    "terminal" => "terminal",
                    "notes" => "notes",
                    "monitor" => "monitor",
                    "solitaire" => "solitaire",
                    "pixels" => "pixels",
                    _ => "clock",
                })
            }
            "close" => Action::CloseFocused,
            "lock" => Action::Lock,
            "help" => Action::Help,
            _ => {
                self.hint = Some(format!("no such command: {cmd} \u{2014} try help"));
                Action::Unknown(cmd)
            }
        }
    }

    // ---- Geometry (shared by draw and hit_test) ----

    fn rect(screen: (i32, i32)) -> (i32, i32, i32, i32) {
        let w = 880.min(screen.0 - 80);
        let h = 74 + 28 + 3 * 40 + 28 + 100 + 46;
        ((screen.0 - w) / 2, screen.1 - 110 - h, w, h)
    }

    fn suggestion_y(py: i32, i: i32) -> i32 {
        py + 74 + 28 + i * 40
    }

    fn app_tile(screen: (i32, i32), i: i32) -> (i32, i32, i32, i32) {
        let (px, py, pw, _) = Self::rect(screen);
        let n = dock::APPS.len() as i32;
        let cell = (pw - 44) / n;
        let ty = py + 74 + 28 + 3 * 40 + 28;
        (px + 22 + i * cell + (cell - 52) / 2, ty, 52, 84)
    }

    pub fn hit_test(&self, pxy: (i32, i32), screen: (i32, i32)) -> Option<LauncherHit> {
        if !self.open {
            return None;
        }
        let (px, py, pw, ph) = Self::rect(screen);
        if !(pxy.0 >= px && pxy.0 < px + pw && pxy.1 >= py && pxy.1 < py + ph) {
            return None;
        }
        for i in 0..SUGGESTIONS.len() as i32 {
            let sy = Self::suggestion_y(py, i);
            if pxy.1 >= sy && pxy.1 < sy + 40 && pxy.0 >= px + 12 && pxy.0 < px + pw - 12 {
                return Some(LauncherHit::Suggestion(i as usize));
            }
        }
        for (i, (name, _, _)) in dock::APPS.iter().enumerate() {
            let (tx, ty, tw, th) = Self::app_tile(screen, i as i32);
            if pxy.0 >= tx && pxy.0 < tx + tw && pxy.1 >= ty && pxy.1 < ty + th {
                return Some(LauncherHit::App(name));
            }
        }
        // Footer lock button.
        let (lx, ly) = (px + pw - 84, py + ph - 37);
        if pxy.0 >= lx && pxy.0 < lx + 64 && pxy.1 >= ly && pxy.1 < ly + 28 {
            return Some(LauncherHit::Lock);
        }
        Some(LauncherHit::Inside)
    }

    pub fn draw(
        &self,
        s: &mut Surface,
        fonts: &mut Fonts,
        backdrop: &Surface,
        screen: (i32, i32),
        now_ms: u64,
    ) {
        if !self.open {
            return;
        }
        let (px, py, pw, ph) = Self::rect(screen);
        s.frosted_panel(backdrop, px, py, pw, ph, RADIUS_PILL, GLASS_TINT);

        // Header: orb + input + kbd chip.
        let oy = py + 20;
        for row in 0..34 {
            let c = lerp(ACC, HUE_VIOLET, (row * 255 / 34) as u32);
            s.fill_rect(px + 22, oy + row, 34, 1, c);
        }
        let (gw, _) = fonts.ui_semibold.measure("*", 16.0);
        fonts
            .ui_semibold
            .draw(s, "*", 16.0, px + 22 + (34 - gw) / 2, oy + 8, ORB_TX);

        let ix = px + 70;
        if self.input.is_empty() {
            fonts.ui.draw(
                s,
                "Ask tinyOS anything \u{2014} run, open, calculate\u{2026}",
                17.0,
                ix,
                oy + 6,
                TX3,
            );
        } else {
            fonts.ui.draw(s, &self.input, 17.0, ix, oy + 6, TX);
        }
        if super::caret_on(now_ms) {
            let (tw, _) = fonts.ui.measure(&self.input, 17.0);
            let caret_x = if self.input.is_empty() { ix } else { ix + tw + 2 };
            s.fill_rect(caret_x, oy + 4, 2, 26, ACC);
        }
        fonts.mono.draw(s, "^K", 11.0, px + pw - 48, oy + 10, TX3);
        s.fill_rect(px + 1, py + 74 - 1, pw - 2, 1, STROKE);

        // Suggested.
        fonts.mono.draw(s, "SUGGESTED", 11.0, px + 22, py + 82, TX3);
        for (i, (label, _)) in SUGGESTIONS.iter().enumerate() {
            let sy = Self::suggestion_y(py, i as i32);
            fonts.ui_semibold.draw(s, "*", 15.0, px + 27, sy + 10, ACC);
            fonts.ui.draw(s, label, 13.5, px + 52, sy + 9, TX);
        }

        // Apps grid.
        let ay = Self::suggestion_y(py, 3);
        fonts.mono.draw(s, "APPS", 11.0, px + 22, ay + 4, TX3);
        for (i, (name, glyph, hue)) in dock::APPS.iter().enumerate() {
            let (tx, ty, tw, _) = Self::app_tile(screen, i as i32);
            s.fill_rounded_rect(tx + (tw - 46) / 2, ty + 6, 46, 46, RADIUS_TILE, CARD2);
            let (gw, _) = fonts.mono.measure(glyph, 15.0);
            fonts
                .mono
                .draw(s, glyph, 15.0, tx + (tw - gw) / 2, ty + 19, *hue);
            let cap = capitalize(name);
            let (cw, _) = fonts.ui.measure(&cap, 11.5);
            fonts
                .ui
                .draw(s, &cap, 11.5, tx + (tw - cw) / 2, ty + 60, TX2);
        }

        // Footer.
        let fy = py + ph - 46;
        s.fill_rect(px + 1, fy, pw - 2, 1, STROKE);
        fonts.mono.draw(
            s,
            "daryl \u{00b7} tinyOS 0.1 \u{201c}meridian\u{201d}",
            11.5,
            px + 22,
            fy + 16,
            TX2,
        );
        let (lx, ly) = (px + pw - 84, fy + 9);
        s.fill_rounded_rect(lx, ly, 64, 28, 8, CARD);
        let (lw, _) = fonts.ui.measure("Lock", 12.0);
        fonts.ui.draw(s, "Lock", 12.0, lx + (64 - lw) / 2, ly + 6, TX2);

        if let Some(hint) = &self.hint {
            fonts.ui.draw(s, hint, 12.0, px + 70, py + 48, TX3);
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
