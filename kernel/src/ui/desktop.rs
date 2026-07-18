//! The desktop shell: wallpaper, menu bar, dock, windows, pointer.

use alloc::string::String;
use alloc::vec::Vec;

use crate::arch::timer;
use crate::drivers::input::{keys, Event, Input, ABS_MAX};
use crate::gfx::font::Fonts;
use crate::gfx::surface::{argb, rgb, with_alpha, Surface};
use crate::gfx::FbInfo;

use super::{cursor, wallpaper};

const MENUBAR_H: i32 = 30;
const TITLEBAR_H: i32 = 34;
const WIN_RADIUS: i32 = 10;
const DOCK_ICON: i32 = 46;
const DOCK_PAD: i32 = 9;

pub const TERM_COLS: usize = 80;
pub const TERM_ROWS: usize = 24;
pub const CELL_W: i32 = 9;
pub const CELL_H: i32 = 19;

pub struct Window {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub title: String,
}

impl Window {
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    fn in_titlebar(&self, px: i32, py: i32) -> bool {
        self.contains(px, py) && py < self.y + TITLEBAR_H
    }

    fn in_close(&self, px: i32, py: i32) -> bool {
        let (cx, cy) = (self.x + 19, self.y + TITLEBAR_H / 2);
        (px - cx) * (px - cx) + (py - cy) * (py - cy) <= 81
    }

    pub fn content_origin(&self) -> (i32, i32) {
        (self.x + 12, self.y + TITLEBAR_H + 4)
    }
}

pub struct Desktop {
    pub wallpaper: Surface,
    pub backdrop: Surface, // blurred wallpaper for frosted panels
    pub window: Option<Window>,
    pub pointer: (i32, i32),
    drag: Option<(i32, i32)>,
    left_down: bool,
    pub shift: bool,
    pub width: i32,
    pub height: i32,
}

pub enum ShellEvent {
    Char(char),
    Key(u16),
}

impl Desktop {
    pub fn new(width: usize, height: usize) -> Self {
        let mut wp = Surface::new(width, height);
        wallpaper::render(&mut wp);
        let backdrop = wp.blurred(10);
        let mut desktop = Self {
            wallpaper: wp,
            backdrop,
            window: None,
            pointer: (width as i32 / 2, height as i32 / 2),
            drag: None,
            left_down: false,
            shift: false,
            width: width as i32,
            height: height as i32,
        };
        desktop.open_terminal();
        desktop
    }

    pub fn open_terminal(&mut self) {
        if self.window.is_none() {
            let w = TERM_COLS as i32 * CELL_W + 24;
            let h = TERM_ROWS as i32 * CELL_H + TITLEBAR_H + 16;
            self.window = Some(Window {
                x: (self.width - w) / 2,
                y: (self.height - h) / 2 + 8,
                w,
                h,
                title: String::from("Terminal"),
            });
        }
    }

    fn dock_rect(&self) -> (i32, i32, i32, i32) {
        let w = DOCK_ICON + DOCK_PAD * 2;
        let h = DOCK_ICON + DOCK_PAD * 2;
        ((self.width - w) / 2, self.height - h - 8, w, h)
    }

    /// Process raw input events; emits key/char events for the focused app.
    pub fn handle(&mut self, events: &[Event], out: &mut Vec<ShellEvent>) {
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
                        self.drag = None;
                    }
                    self.left_down = down;
                }
                Event::Button { right: true, .. } => {}
                Event::Key { code, down } => {
                    if code == keys::LSHIFT || code == keys::RSHIFT {
                        self.shift = down;
                    } else if down && self.window.is_some() {
                        match crate::drivers::input::keycode_to_char(code, self.shift) {
                            Some(c) => out.push(ShellEvent::Char(c)),
                            None => out.push(ShellEvent::Key(code)),
                        }
                    }
                }
                Event::Frame => {}
            }
        }

        // Window dragging follows the pointer.
        if let (Some((dx, dy)), Some(win)) = (self.drag, self.window.as_mut()) {
            win.x = (self.pointer.0 - dx).clamp(-win.w + 60, self.width - 60);
            win.y = (self.pointer.1 - dy).clamp(MENUBAR_H, self.height - TITLEBAR_H);
        }
    }

    fn on_click(&mut self) {
        let (px, py) = self.pointer;

        if let Some(win) = &self.window {
            if win.in_close(px, py) {
                self.window = None;
                return;
            }
            if win.in_titlebar(px, py) {
                self.drag = Some((px - win.x, py - win.y));
                return;
            }
        }

        let (dx, dy, dw, dh) = self.dock_rect();
        if px >= dx && px < dx + dw && py >= dy && py < dy + dh {
            self.open_terminal();
        }
    }

    /// Compose one frame. `content` draws the focused window's interior.
    pub fn compose(
        &mut self,
        surface: &mut Surface,
        fonts: &mut Fonts,
        content: impl FnOnce(&mut Surface, &mut Fonts, &Window),
    ) {
        surface.copy_from(&self.wallpaper);

        if self.window.is_some() {
            self.draw_window(surface);
            self.draw_window_title(surface, fonts);
            let win = self.window.as_ref().unwrap();
            content(surface, fonts, win);
        }

        self.draw_menubar(surface, fonts);
        self.draw_dock(surface, fonts);
        cursor::draw(surface, self.pointer.0, self.pointer.1);
    }

    fn draw_menubar(&self, surface: &mut Surface, fonts: &mut Fonts) {
        surface.frosted_panel(
            &self.backdrop,
            0,
            -8,
            self.width,
            MENUBAR_H + 8,
            8,
            argb(120, 18, 18, 30),
        );

        // Logo mark: small rounded square with a "t".
        surface.fill_rounded_rect(14, 7, 17, 17, 5, rgb(235, 235, 245));
        fonts.mono.draw(surface, "t", 14.0, 19, 8, rgb(30, 28, 50));

        fonts
            .ui_semibold
            .draw(surface, "tinyOS", 15.0, 42, 7, rgb(245, 245, 250));
        if self.window.is_some() {
            fonts
                .ui_medium
                .draw(surface, "Terminal", 15.0, 110, 7, argb(210, 240, 240, 250));
        }

        // Iconic 9:41, advanced by uptime.
        let mins = 9 * 60 + 41 + timer::uptime_ms() / 60_000;
        let clock = alloc::format!("{}:{:02}", mins / 60 % 24, mins % 60);
        let (cw, _) = fonts.ui_medium.measure(&clock, 15.0);
        fonts
            .ui_medium
            .draw(surface, &clock, 15.0, self.width - cw - 16, 7, rgb(245, 245, 250));
    }

    fn draw_dock(&self, surface: &mut Surface, fonts: &mut Fonts) {
        let (dx, dy, dw, dh) = self.dock_rect();
        surface.frosted_panel(&self.backdrop, dx, dy, dw, dh, 16, argb(96, 30, 30, 44));

        // Terminal icon: dark rounded square with a prompt glyph.
        let ix = dx + DOCK_PAD;
        let iy = dy + DOCK_PAD;
        surface.fill_rounded_rect(ix, iy, DOCK_ICON, DOCK_ICON, 11, rgb(28, 28, 36));
        surface.fill_rounded_rect(ix, iy, DOCK_ICON, 2, 1, argb(70, 255, 255, 255));
        fonts
            .mono
            .draw(surface, ">_", 17.0, ix + 8, iy + 12, rgb(120, 230, 190));

        // Running indicator.
        if self.window.is_some() {
            surface.fill_rounded_rect(dx + dw / 2 - 2, dy + dh - 5, 4, 4, 2, argb(200, 240, 240, 255));
        }
    }

    fn draw_window(&self, surface: &mut Surface) {
        let win = self.window.as_ref().unwrap();

        // Soft shadow: stacked translucent rounded rects.
        for i in (0..14).step_by(2) {
            surface.fill_rounded_rect(
                win.x - i,
                win.y - i + 6,
                win.w + 2 * i,
                win.h + 2 * i,
                WIN_RADIUS + i,
                argb(9, 0, 0, 0),
            );
        }

        // Body and title bar.
        surface.fill_rounded_rect(win.x, win.y, win.w, win.h, WIN_RADIUS, rgb(30, 30, 38));
        surface.fill_rounded_rect(win.x, win.y, win.w, TITLEBAR_H + WIN_RADIUS, WIN_RADIUS, rgb(52, 52, 62));
        surface.fill_rect(win.x, win.y + TITLEBAR_H, win.w, win.h - TITLEBAR_H - WIN_RADIUS, rgb(30, 30, 38));
        // Rounded bottom of content area.
        surface.fill_rounded_rect(
            win.x,
            win.y + win.h - 2 * WIN_RADIUS,
            win.w,
            2 * WIN_RADIUS,
            WIN_RADIUS,
            rgb(30, 30, 38),
        );
        surface.fill_rounded_rect(win.x, win.y, win.w, 2, 1, argb(50, 255, 255, 255));

        // Traffic lights.
        let cy = win.y + TITLEBAR_H / 2 - 6;
        for (i, color) in [rgb(255, 95, 87), rgb(254, 188, 46), rgb(40, 200, 64)]
            .iter()
            .enumerate()
        {
            surface.fill_rounded_rect(win.x + 13 + i as i32 * 20, cy, 12, 12, 6, *color);
        }
    }

    fn draw_window_title(&self, surface: &mut Surface, fonts: &mut Fonts) {
        if let Some(win) = &self.window {
            let (tw, _) = fonts.ui_medium.measure(&win.title, 14.0);
            fonts.ui_medium.draw(
                surface,
                &win.title,
                14.0,
                win.x + (win.w - tw) / 2,
                win.y + 9,
                argb(220, 235, 235, 245),
            );
        }
    }
}
