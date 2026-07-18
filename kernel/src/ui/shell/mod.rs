//! Shell v2: modern soft-dark-neutral windowed desktop.

pub mod app;
pub mod dock;
pub mod statusbar;
pub mod tokens;
pub mod wallpaper;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::arch::timer;
use crate::drivers::input::{keycode_to_char, keys, Event, ABS_MAX};
use crate::gfx::font::Fonts;
use crate::gfx::surface::{argb, with_alpha, Surface};

use self::app::{App, Rect};
use self::tokens::*;
use super::cursor;

pub struct Window {
    pub rect: Rect,
    pub app: Box<dyn App>,
    /// Pre-snap geometry; Some(..) while snapped or maximized.
    pub restore: Option<Rect>,
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
    fn close_center(&self) -> (i32, i32) {
        (self.rect.x + self.rect.w - 24, self.rect.y + TITLE_H / 2)
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
                x: x.clamp(8, self.width - pw - 8),
                y: y.clamp(STATUS_H + 8, self.height - ph - 8),
                w: pw,
                h: ph,
            },
            app,
            restore: None,
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
                    win.rect.y = (pointer.1 - dy).clamp(STATUS_H, height - TITLE_H);
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
            let win = &self.windows[i];
            if !win.rect.contains(px, py) {
                continue;
            }
            let (cx, cy) = win.close_center();
            if (px - cx) * (px - cx) + (py - cy) * (py - cy) <= 144 {
                self.windows.remove(i);
                if self.focus >= self.windows.len() {
                    self.focus = self.windows.len().saturating_sub(1);
                }
                return;
            }
            // Bring to front (stack order = vec order).
            let win = self.windows.remove(i);
            self.windows.push(win);
            self.focus = self.windows.len() - 1;
            let r = self.windows[self.focus].rect;
            if px >= r.x + r.w - 18 && py >= r.y + r.h - 18 {
                // Resize grip: remember pointer offset from the corner.
                self.drag = Some((r.x + r.w - px, r.y + r.h - py, DragKind::Resize));
            } else if py < r.y + TITLE_H {
                self.drag = Some((px - r.x, py - r.y, DragKind::Move));
            }
            return;
        }

        if let Some(name) = dock::hit_test((px, py), (self.width, self.height)) {
            self.open_named(name);
        }
    }

    fn zone_rect(&self, zone: SnapZone) -> Rect {
        let top = STATUS_H + 8;
        let bottom = self.height - 90; // keep clear of the dock band
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
        } else if py < STATUS_H + 8 {
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
                r.y = r.y.clamp(STATUS_H + 8, (height - r.h - 8).max(STATUS_H + 8));
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
            let already_focused = i == self.focus;
            if !(name == "notes" && already_focused) {
                let win = self.windows.remove(i);
                self.windows.push(win);
                self.focus = self.windows.len() - 1;
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
        if self.ctrl {
            match code {
                keys::LEFT => self.snap_focused(SnapZone::Left),
                keys::RIGHT => self.snap_focused(SnapZone::Right),
                keys::UP => self.snap_focused(SnapZone::Max),
                keys::DOWN => self.restore_focused(),
                _ => {}
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
        for i in 0..self.windows.len() {
            draw_window(s, fonts, &mut self.windows[i], i == focus, now);
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

        statusbar::draw(s, fonts, &self.backdrop, self.width);
        let running: Vec<(&str, bool)> = dock::APPS
            .iter()
            .map(|&(name, _)| {
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
        cursor::draw(s, self.pointer.0, self.pointer.1);
    }
}

/// Modern window chrome: soft shadow, unified surface, inline title row,
/// ghost close button on the right, accent border when focused.
fn draw_window(s: &mut Surface, fonts: &mut Fonts, win: &mut Window, focused: bool, now: u64) {
    let r = win.rect;

    // Soft diffuse shadow.
    for i in 0..5 {
        let spread = 3 * (i + 1);
        s.fill_rounded_rect(
            r.x - spread / 2,
            r.y - spread / 2 + 4,
            r.w + spread,
            r.h + spread,
            RADIUS + spread / 2,
            argb(8, 0, 0, 0),
        );
    }

    s.fill_rounded_rect(r.x, r.y, r.w, r.h, RADIUS, SURFACE);

    // Border: accent when focused (2px), hairline otherwise.
    let border = if focused { with_alpha(ACCENT, 230) } else { BORDER };
    let t = if focused { 2 } else { 1 };
    s.fill_rect(r.x + RADIUS, r.y, r.w - 2 * RADIUS, t, border);
    s.fill_rect(r.x + RADIUS, r.y + r.h - t, r.w - 2 * RADIUS, t, border);
    s.fill_rect(r.x, r.y + RADIUS, t, r.h - 2 * RADIUS, border);
    s.fill_rect(r.x + r.w - t, r.y + RADIUS, t, r.h - 2 * RADIUS, border);

    // Inline title row: glyph + title left, ghost close right.
    fonts
        .mono
        .draw(s, win.app.glyph(), 13.0, r.x + 16, r.y + 10, TEXT_DIM);
    let gx = r.x + 16 + fonts.mono.measure(win.app.glyph(), 13.0).0 + 10;
    let title = alloc::string::String::from(win.app.title());
    fonts.ui_medium.draw(s, &title, 15.0, gx, r.y + 8, TEXT);

    let (cx, cy) = win.close_center();
    s.fill_rounded_rect(cx - 11, cy - 11, 22, 22, 11, SURFACE_HI);
    let (xw, _) = fonts.mono.measure("x", 13.0);
    fonts
        .mono
        .draw(s, "x", 13.0, cx - xw / 2, cy - 9, TEXT_DIM);

    // Resize grip affordance: two short diagonal ticks.
    for i in 0..2 {
        let off = 6 + i * 4;
        s.fill_rect(r.x + r.w - off - 4, r.y + r.h - 7, off, 2, with_alpha(TEXT_DIM, 130));
    }

    let body = win.body();
    win.app.draw(s, fonts, body, focused, now);
}
