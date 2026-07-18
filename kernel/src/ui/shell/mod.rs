//! Shell v2: modern soft-dark-neutral windowed desktop.

pub mod app;
pub mod calc;
pub mod clockpill;
pub mod dock;
pub mod palette;
pub mod tokens;
pub mod wallpaper;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::arch::timer;
use crate::drivers::input::{keycode_to_char, keys, Event, ABS_MAX};
use crate::gfx::font::Fonts;
use crate::gfx::surface::{argb, with_alpha, Surface};

use self::app::{App, Rect};
use self::palette::{Action, Palette};
use self::tokens::*;
use super::cursor;

pub struct Window {
    pub rect: Rect,
    pub app: Box<dyn App>,
    /// Pre-snap geometry; Some(..) while snapped or maximized.
    pub restore: Option<Rect>,
    /// Minimized to the dock.
    pub hidden: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Control {
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Copy, PartialEq)]
enum SnapZone {
    Left,
    Right,
    Max,
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Move,
    Resize,
}

impl Window {
    /// Control glyph centers, right-aligned in the title row: - [] x.
    fn control_center(&self, c: Control) -> (i32, i32) {
        let cy = self.rect.y + TITLE_H / 2;
        let cx = match c {
            Control::Close => self.rect.x + self.rect.w - 26,
            Control::Maximize => self.rect.x + self.rect.w - 56,
            Control::Minimize => self.rect.x + self.rect.w - 86,
        };
        (cx, cy)
    }

    fn control_hit(&self, px: i32, py: i32) -> Option<Control> {
        for c in [Control::Minimize, Control::Maximize, Control::Close] {
            let (cx, cy) = self.control_center(c);
            if (px - cx).abs() <= 13 && (py - cy).abs() <= 15 {
                return Some(c);
            }
        }
        None
    }

    fn body(&self) -> Rect {
        Rect {
            x: self.rect.x + 14,
            y: self.rect.y + TITLE_H + 6,
            w: self.rect.w - 28,
            h: self.rect.h - TITLE_H - 20,
        }
    }
}

pub struct Shell {
    width: i32,
    height: i32,
    wallpaper: Surface,
    backdrop: Surface,
    windows: Vec<Window>,
    focus: usize,
    pointer: (i32, i32),
    shift: bool,
    ctrl: bool,
    left_down: bool,
    drag: Option<(i32, i32, DragKind)>,
    palette: Palette,
    quick_open: bool,
}

impl Shell {
    pub fn new(width: usize, height: usize) -> Self {
        let mut wp = Surface::new(width, height);
        wallpaper::render(&mut wp);
        let backdrop = wp.blurred(10);
        let mut shell = Self {
            width: width as i32,
            height: height as i32,
            wallpaper: wp,
            backdrop,
            windows: Vec::new(),
            focus: 0,
            pointer: (width as i32 / 2, height as i32 / 2),
            shift: false,
            ctrl: false,
            left_down: false,
            drag: None,
            palette: Palette::new(),
            quick_open: false,
        };
        shell.open(Box::new(crate::apps::terminal::TerminalApp::new()));
        shell
    }

    pub fn open(&mut self, app: Box<dyn App>) {
        let (pw, ph) = app.preferred_size(self.width, self.height);
        let n = self.windows.len() as i32;
        let x = (self.width - pw) / 2 + n * 32 - 48;
        let y = (self.height - ph) / 2 + n * 24 - 24;
        self.windows.push(Window {
            rect: Rect {
                x: x.clamp(8, (self.width - pw - 8).max(8)),
                y: y.clamp(16, (self.height - ph - 8).max(16)),
                w: pw,
                h: ph,
            },
            app,
            restore: None,
            hidden: false,
        });
        self.focus = self.windows.len() - 1;
    }

    pub fn handle(&mut self, events: &[Event]) {
        for ev in events {
            match *ev {
                Event::PointerX(v) => {
                    self.pointer.0 = (v * (self.width as u32 - 1) / ABS_MAX) as i32
                }
                Event::PointerY(v) => {
                    self.pointer.1 = (v * (self.height as u32 - 1) / ABS_MAX) as i32
                }
                Event::Button { right: false, down } => {
                    if down && !self.left_down {
                        self.on_click();
                    }
                    if !down {
                        if let Some((_, _, DragKind::Move)) = self.drag {
                            if let Some(zone) = self.hover_zone() {
                                self.snap_focused(zone);
                            }
                        }
                        self.drag = None;
                    }
                    self.left_down = down;
                }
                Event::Button { right: true, .. } => {}
                Event::Key { code, down } => match code {
                    keys::LSHIFT | keys::RSHIFT => self.shift = down,
                    keys::LCTRL => self.ctrl = down,
                    _ if down => self.on_key_down(code),
                    _ => {}
                },
                Event::Frame => {}
            }
        }

        let pointer = self.pointer;
        let (width, height) = (self.width, self.height);
        if let (Some((dx, dy, kind)), Some(win)) = (self.drag, self.windows.get_mut(self.focus)) {
            match kind {
                DragKind::Move => {
                    // Moving a snapped window restores it under the pointer.
                    if let Some(r) = win.restore.take() {
                        win.rect.w = r.w;
                        win.rect.h = r.h;
                    }
                    win.rect.x = (pointer.0 - dx).clamp(-win.rect.w + 80, width - 80);
                    win.rect.y = (pointer.1 - dy).clamp(4, height - TITLE_H);
                }
                DragKind::Resize => {
                    let (mw, mh) = win.app.min_size();
                    win.rect.w = (pointer.0 - win.rect.x + dx).clamp(mw, width - win.rect.x - 4);
                    win.rect.h = (pointer.1 - win.rect.y + dy).clamp(mh, height - win.rect.y - 4);
                }
            }
        }
    }

    fn on_click(&mut self) {
        let (px, py) = self.pointer;

        for i in (0..self.windows.len()).rev() {
            if self.windows[i].hidden || !self.windows[i].rect.contains(px, py) {
                continue;
            }
            if let Some(control) = self.windows[i].control_hit(px, py) {
                match control {
                    Control::Close => {
                        self.windows.remove(i);
                        self.focus_topmost_visible();
                    }
                    Control::Minimize => {
                        self.windows[i].hidden = true;
                        self.focus_topmost_visible();
                    }
                    Control::Maximize => {
                        self.bring_to_front(i);
                        if self.windows[self.focus].restore.is_some() {
                            self.restore_focused();
                        } else {
                            self.snap_focused(SnapZone::Max);
                        }
                    }
                }
                return;
            }
            self.bring_to_front(i);
            let r = self.windows[self.focus].rect;
            if px >= r.x + r.w - 18 && py >= r.y + r.h - 18 {
                self.drag = Some((r.x + r.w - px, r.y + r.h - py, DragKind::Resize));
            } else if py < r.y + TITLE_H {
                self.drag = Some((px - r.x, py - r.y, DragKind::Move));
            }
            return;
        }

        match dock::hit_test((px, py), (self.width, self.height)) {
            Some(dock::DockHit::Orb) => {
                if self.palette.open {
                    self.palette.dismiss()
                } else {
                    self.palette.summon()
                }
                return;
            }
            Some(dock::DockHit::App(name)) => {
                self.open_named(name);
                return;
            }
            None => {}
        }
        if clockpill::hit_test((px, py), (self.width, self.height)) {
            self.quick_open = !self.quick_open;
        }
    }

    fn bring_to_front(&mut self, i: usize) {
        let win = self.windows.remove(i);
        self.windows.push(win);
        self.focus = self.windows.len() - 1;
    }

    fn focus_topmost_visible(&mut self) {
        self.focus = self
            .windows
            .iter()
            .rposition(|w| !w.hidden)
            .unwrap_or(0);
    }

    fn zone_rect(&self, zone: SnapZone) -> Rect {
        let top = 16;
        let bottom = self.height - 100; // keep clear of the dock band
        match zone {
            SnapZone::Left => Rect { x: 8, y: top, w: self.width / 2 - 12, h: bottom - top },
            SnapZone::Right => Rect {
                x: self.width / 2 + 4,
                y: top,
                w: self.width / 2 - 12,
                h: bottom - top,
            },
            SnapZone::Max => Rect { x: 16, y: top, w: self.width - 32, h: bottom - top },
        }
    }

    fn hover_zone(&self) -> Option<SnapZone> {
        let (px, py) = self.pointer;
        if px < 16 {
            Some(SnapZone::Left)
        } else if px >= self.width - 16 {
            Some(SnapZone::Right)
        } else if py < 12 {
            Some(SnapZone::Max)
        } else {
            None
        }
    }

    fn snap_focused(&mut self, zone: SnapZone) {
        let rect = self.zone_rect(zone);
        if let Some(win) = self.windows.get_mut(self.focus) {
            if win.restore.is_none() {
                win.restore = Some(win.rect);
            }
            win.rect = rect;
        }
    }

    fn restore_focused(&mut self) {
        let (width, height) = (self.width, self.height);
        if let Some(win) = self.windows.get_mut(self.focus) {
            if let Some(mut r) = win.restore.take() {
                // The snapshot may have been taken mid-drag at a screen
                // edge; always restore fully on-screen.
                r.x = r.x.clamp(8, (width - r.w - 8).max(8));
                r.y = r.y.clamp(16, (height - r.h - 8).max(16));
                win.rect = r;
            }
        }
    }

    pub fn open_named(&mut self, name: &str) {
        // Notes: focus most recent if open and unfocused; open another if
        // it's already focused (or none open). Others: single instance.
        let existing = self.windows.iter().rposition(|w| {
            let t = w.app.title();
            t.eq_ignore_ascii_case(name)
                || (name == "clock" && t == "Timer")
        });
        if let Some(i) = existing {
            let already_focused = i == self.focus && !self.windows[i].hidden;
            if !(name == "notes" && already_focused) {
                self.windows[i].hidden = false;
                self.bring_to_front(i);
                return;
            }
        }
        match name {
            "terminal" => self.open(Box::new(crate::apps::terminal::TerminalApp::new())),
            "notes" => self.open(Box::new(crate::apps::notes::NotesApp::new())),
            "monitor" => self.open(Box::new(crate::apps::monitor::MonitorApp::new())),
            "clock" => self.open(Box::new(crate::apps::clock::ClockApp::new())),
            _ => {}
        }
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Open(name) => {
                self.open_named(name);
                self.palette.dismiss();
            }
            Action::CloseFocused => {
                if !self.windows.is_empty() {
                    let i = self.focus;
                    self.windows.remove(i);
                    self.focus = self.windows.len().saturating_sub(1);
                }
                self.palette.dismiss();
            }
            Action::Help => {
                self.palette.hint = Some(alloc::string::String::from(
                    "apps: terminal notes monitor clock | = expr | timer 5m | close",
                ));
            }
            Action::Calc(expr) => {
                self.palette.hint = Some(match calc::eval(&expr) {
                    Some(v) if v.abs() < 1e15 && v == (v as i64) as f64 => {
                        alloc::format!("= {}", v as i64)
                    }
                    Some(v) => alloc::format!("= {v}"),
                    None => alloc::string::String::from("can't evaluate"),
                });
            }
            Action::Timer(secs) => {
                self.open_named("clock");
                if let Some(win) = self.windows.get_mut(self.focus) {
                    if let Some(clock) = win
                        .app
                        .as_any()
                        .downcast_mut::<crate::apps::clock::ClockApp>()
                    {
                        clock.start_timer(secs);
                    }
                }
                self.palette.dismiss();
            }
            Action::Dismiss => self.palette.dismiss(),
            Action::None | Action::Unknown(_) => {}
        }
    }

    /// Per-frame stats feed for any open monitor window.
    pub fn stats_tick(&mut self, events: u32) {
        let now = timer::uptime_ms();
        for win in &mut self.windows {
            if let Some(mon) = win
                .app
                .as_any()
                .downcast_mut::<crate::apps::monitor::MonitorApp>()
            {
                mon.tick(now, events);
            }
        }
    }

    fn on_key_down(&mut self, code: u16) {
        const KEY_K: u16 = 37;
        if self.ctrl {
            match code {
                KEY_K => {
                    if self.palette.open {
                        self.palette.dismiss()
                    } else {
                        self.palette.summon()
                    }
                }
                keys::LEFT => self.snap_focused(SnapZone::Left),
                keys::RIGHT => self.snap_focused(SnapZone::Right),
                keys::UP => self.snap_focused(SnapZone::Max),
                keys::DOWN => self.restore_focused(),
                _ => {}
            }
            return;
        }
        if self.palette.open {
            match code {
                keys::ESC => self.palette.dismiss(),
                keys::ENTER => {
                    let action = self.palette.submit();
                    self.act(action);
                }
                _ => match keycode_to_char(code, self.shift) {
                    Some(c) => self.palette.on_char(c),
                    None => self.palette.on_key(code),
                },
            }
            return;
        }
        if let Some(win) = self.windows.get_mut(self.focus) {
            match keycode_to_char(code, self.shift) {
                Some(c) => win.app.on_char(c),
                None => win.app.on_key(code),
            }
        }
    }

    pub fn compose(&mut self, s: &mut Surface, fonts: &mut Fonts) {
        s.copy_from(&self.wallpaper);
        let now = timer::uptime_ms();

        let focus = self.focus;
        let backdrop = &self.backdrop as *const Surface;
        for i in 0..self.windows.len() {
            if self.windows[i].hidden {
                continue;
            }
            // SAFETY: backdrop is only read while windows are drawn.
            draw_window(s, fonts, unsafe { &*backdrop }, &mut self.windows[i], i == focus, now);
        }

        // Snap preview while dragging near an edge.
        if matches!(self.drag, Some((_, _, DragKind::Move))) {
            if let Some(zone) = self.hover_zone() {
                let z = self.zone_rect(zone);
                let a = with_alpha(ACCENT, 200);
                s.fill_rounded_rect(z.x, z.y, z.w, z.h, RADIUS, with_alpha(ACCENT, 36));
                s.fill_rect(z.x + RADIUS, z.y, z.w - 2 * RADIUS, 2, a);
                s.fill_rect(z.x + RADIUS, z.y + z.h - 2, z.w - 2 * RADIUS, 2, a);
                s.fill_rect(z.x, z.y + RADIUS, 2, z.h - 2 * RADIUS, a);
                s.fill_rect(z.x + z.w - 2, z.y + RADIUS, 2, z.h - 2 * RADIUS, a);
            }
        }


        let running: Vec<(&str, bool)> = dock::APPS
            .iter()
            .map(|&(name, _, _)| {
                (
                    name,
                    self.windows.iter().any(|w| {
                        let t = w.app.title();
                        t.eq_ignore_ascii_case(name) || (name == "clock" && t == "Timer")
                    }),
                )
            })
            .collect();
        dock::draw(s, fonts, &self.backdrop, (self.width, self.height), &running);
        clockpill::draw(s, fonts, &self.backdrop, (self.width, self.height));
        self.palette.draw(s, fonts, self.width, now);
        cursor::draw(s, self.pointer.0, self.pointer.1);
    }
}

/// Meridian window chrome: glass surface (frosted backdrop + tint),
/// hairline border (stroke2 when focused), inline title row with app-hued
/// glyph, and mono text controls - [] x on the right.
fn draw_window(
    s: &mut Surface,
    fonts: &mut Fonts,
    backdrop: &Surface,
    win: &mut Window,
    focused: bool,
    now: u64,
) {
    let r = win.rect;

    // One huge soft shadow.
    for i in 0..6 {
        let spread = 4 * (i + 1);
        s.fill_rounded_rect(
            r.x - spread / 2,
            r.y - spread / 2 + 8,
            r.w + spread,
            r.h + spread,
            RADIUS + spread / 2,
            argb(9, 0, 0, 0),
        );
    }

    // Glass body.
    s.frosted_panel(backdrop, r.x, r.y, r.w, r.h, RADIUS, WIN_TINT);

    let border = if focused { STROKE2 } else { STROKE };
    s.fill_rect(r.x + RADIUS, r.y, r.w - 2 * RADIUS, 1, border);
    s.fill_rect(r.x + RADIUS, r.y + r.h - 1, r.w - 2 * RADIUS, 1, border);
    s.fill_rect(r.x, r.y + RADIUS, 1, r.h - 2 * RADIUS, border);
    s.fill_rect(r.x + r.w - 1, r.y + RADIUS, 1, r.h - 2 * RADIUS, border);
    // Title row hairline.
    s.fill_rect(r.x + 1, r.y + TITLE_H, r.w - 2, 1, STROKE);

    // Glyph in the app's hue, then the title.
    let hue = dock::APPS
        .iter()
        .find(|(n, _, _)| win.app.title().eq_ignore_ascii_case(n) || (*n == "clock" && win.app.title() == "Timer"))
        .map(|&(_, _, h)| h)
        .unwrap_or(ACC);
    fonts
        .mono
        .draw(s, win.app.glyph(), 12.0, r.x + 16, r.y + 14, hue);
    let gx = r.x + 16 + fonts.mono.measure(win.app.glyph(), 12.0).0 + 10;
    let title = alloc::string::String::from(win.app.title());
    fonts.ui_semibold.draw(s, &title, 13.0, gx, r.y + 13, TX);

    // Controls: - [] x (mono, dim).
    for (c, glyph) in [
        (Control::Minimize, "-"),
        (Control::Maximize, "[]"),
        (Control::Close, "x"),
    ] {
        let (cx, cy) = win.control_center(c);
        let (gw, _) = fonts.mono.measure(glyph, 13.0);
        fonts.mono.draw(s, glyph, 13.0, cx - gw / 2, cy - 9, TX3);
    }

    // Resize grip affordance.
    for i in 0..2 {
        let off = 6 + i * 4;
        s.fill_rect(r.x + r.w - off - 4, r.y + r.h - 7, off, 2, with_alpha(TX3, 150));
    }

    let body = win.body();
    win.app.draw(s, fonts, body, focused, now);
}
