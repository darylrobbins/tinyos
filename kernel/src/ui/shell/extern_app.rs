//! Host for external (userspace) windowed apps.
//!
//! A third-party process opens a window over its shell channel using the
//! app-api window protocol (see `apps/sdk/src/window.rs`). The terminal
//! `run` command hands the kernel end of that channel here; the shell turns
//! an `OPEN` message into a real Meridian window whose `App` maps the app's
//! shared BGRA surface and blits it each frame, forwards input, and requests
//! close. The surface is identity-mapped (VA == PA), so the shell reads the
//! app's pixels directly from the MemObj's physical address — zero copy.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use spin::Mutex;

use super::app::{App, Rect};
use crate::gfx::font::Fonts;
use crate::gfx::surface::Surface;
use crate::obj::channel::{ChannelEnd, Message};
use crate::obj::memobj::MemObj;
use crate::obj::syscall::ST_SHOULD_WAIT;
use crate::obj::Object;

// Window protocol opcodes — mirror apps/sdk/src/window.rs.
use abi::window::{
    OP_BUTTON, OP_CHAR, OP_CLOSE_REQ, OP_CTRL, OP_KEY, OP_OPEN, OP_OPENED, OP_POINTER, OP_PRESENT,
};

/// A spawned userspace process that may open a window. Handed from the
/// terminal to the shell via `SPAWN_QUEUE`.
pub struct PendingApp {
    pub shell: Arc<ChannelEnd>,
    pub name: String,
}

static SPAWN_QUEUE: Mutex<Vec<PendingApp>> = Mutex::new(Vec::new());

/// Terminal side: register a spawned app so the shell can host its window.
pub fn register(shell: Arc<ChannelEnd>, name: String) {
    SPAWN_QUEUE.lock().push(PendingApp { shell, name });
}

/// Shell side: take newly-spawned apps awaiting their first `OPEN`.
pub fn take_pending() -> Vec<PendingApp> {
    core::mem::take(&mut *SPAWN_QUEUE.lock())
}

/// Are apps waiting to be hosted? The shell keeps waking while so, so a slow
/// app's `OPEN` is serviced even after the interactive window lapses.
pub fn has_pending() -> bool {
    !SPAWN_QUEUE.lock().is_empty()
}

fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

pub enum OpenResult {
    /// No `OPEN` yet; keep this app pending.
    Waiting,
    /// The app opened a window; host it.
    Opened(ExternApp),
    /// The app exited before opening a window; drop it.
    Done,
}

/// A hosted window backed by an app's shared BGRA surface.
pub struct ExternApp {
    ch: Arc<ChannelEnd>,
    title: String,
    w: u32,
    h: u32,
    surface_pa: usize,
    /// Keep the surface alive for as long as we might blit it, even if the
    /// app's process exits mid-frame.
    _surface: Arc<MemObj>,
    /// Completed frame captured from the surface on `PRESENT`. Blitting this
    /// instead of the live surface keeps half-drawn app frames off screen.
    snapshot: Vec<u32>,
    closed: bool,
}

impl ExternApp {
    /// Advance a pending app: consume queued protocol messages until its
    /// `OPEN` arrives (→ `Opened`), the queue drains (→ `Waiting`), or the
    /// peer closes first (→ `Done`).
    pub fn try_open(pending: &PendingApp) -> OpenResult {
        loop {
            let msg = match pending.shell.recv() {
                Ok(m) => m,
                Err(ST_SHOULD_WAIT) => return OpenResult::Waiting,
                Err(_) => return OpenResult::Done, // peer closed before opening
            };
            if msg.bytes.len() < 16 || le_u32(&msg.bytes, 0) != OP_OPEN {
                continue; // ignore stray messages before OPEN
            }
            let w = le_u32(&msg.bytes, 4);
            let h = le_u32(&msg.bytes, 8);
            let tlen = le_u32(&msg.bytes, 12) as usize;
            let title = msg
                .bytes
                .get(16..16 + tlen)
                .and_then(|s| core::str::from_utf8(s).ok())
                .unwrap_or(&pending.name)
                .to_string();
            // The surface rides as the first moved MemObj handle.
            let Some(surface) = msg.handles.iter().find_map(|h| match &h.object {
                Object::MemObj(m) => Some(m.clone()),
                _ => None,
            }) else {
                continue; // malformed OPEN without a surface: ignore
            };
            if w == 0 || h == 0 || surface.size() < w as usize * h as usize * 4 {
                continue; // surface too small for the claimed geometry
            }
            let surface_pa = surface.pa();
            let _ = pending.shell.send(Message {
                bytes: OP_OPENED.to_le_bytes().to_vec(),
                handles: Vec::new(),
            });
            return OpenResult::Opened(ExternApp {
                ch: pending.shell.clone(),
                title,
                w,
                h,
                surface_pa,
                _surface: surface,
                snapshot: Vec::new(),
                closed: false,
            });
        }
    }

    /// Drain protocol messages from the app; returns true once it closes
    /// (peer gone). `PRESENT` snapshots the surface as the completed frame.
    pub fn pump(&mut self) -> bool {
        let mut present = false;
        loop {
            match self.ch.recv() {
                Ok(m) => {
                    if m.bytes.len() >= 4 && le_u32(&m.bytes, 0) == OP_PRESENT {
                        present = true;
                    }
                }
                Err(ST_SHOULD_WAIT) => break,
                Err(_) => {
                    self.closed = true;
                    break;
                }
            }
        }
        if present {
            let src = unsafe {
                core::slice::from_raw_parts(
                    self.surface_pa as *const u32,
                    (self.w * self.h) as usize,
                )
            };
            if self.snapshot.len() != src.len() {
                self.snapshot = src.to_vec();
            } else {
                self.snapshot.copy_from_slice(src);
            }
        }
        self.closed
    }

    fn send(&self, bytes: Vec<u8>) {
        let _ = self.ch.send(Message { bytes, handles: Vec::new() });
    }
}

impl App for ExternApp {
    fn as_any(&mut self) -> &mut dyn core::any::Any {
        self
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn glyph(&self) -> &str {
        "\u{25A3}" // ▣ hosted surface
    }
    fn preferred_size(&self, screen_w: i32, screen_h: i32) -> (i32, i32) {
        // Content is w×h; add the window chrome the shell draws around it.
        (
            (self.w as i32 + 28).min(screen_w - 16),
            (self.h as i32 + TITLE_H_PLUS).min(screen_h - 24),
        )
    }
    fn min_size(&self) -> (i32, i32) {
        (self.w as i32 + 28, self.h as i32 + TITLE_H_PLUS)
    }
    fn wants_frames(&self) -> bool {
        !self.closed
    }
    fn draw(&mut self, s: &mut Surface, _fonts: &mut Fonts, body: Rect, _focused: bool, _now: u64) {
        // Blit the last presented frame 1:1, clipped to the window body.
        // Before the first PRESENT, fall back to the live surface.
        let src: &[u32] = if self.snapshot.is_empty() {
            unsafe {
                core::slice::from_raw_parts(
                    self.surface_pa as *const u32,
                    (self.w * self.h) as usize,
                )
            }
        } else {
            &self.snapshot
        };
        let cw = (self.w as i32).min(body.w.max(0));
        let ch = (self.h as i32).min(body.h.max(0));
        for y in 0..ch {
            let row = y as usize * self.w as usize;
            for x in 0..cw {
                s.put(body.x + x, body.y + y, src[row + x as usize]);
            }
        }
    }
    fn on_char(&mut self, c: char) {
        let mut b = OP_CHAR.to_le_bytes().to_vec();
        b.extend_from_slice(&(c as u32).to_le_bytes());
        self.send(b);
    }
    fn on_key(&mut self, code: u16) {
        let mut b = OP_KEY.to_le_bytes().to_vec();
        b.extend_from_slice(&code.to_le_bytes());
        b.push(1); // key down
        self.send(b);
    }
    fn on_ctrl_key(&mut self, code: u16) {
        let mut b = OP_CTRL.to_le_bytes().to_vec();
        b.extend_from_slice(&(code as u32).to_le_bytes());
        self.send(b);
    }
    fn wants_pointer(&self) -> bool {
        true
    }
    fn on_pointer_move(&mut self, x: i32, y: i32) {
        let mut b = OP_POINTER.to_le_bytes().to_vec();
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
        self.send(b);
    }
    fn on_button(&mut self, down: bool, x: i32, y: i32) {
        let mut b = OP_BUTTON.to_le_bytes().to_vec();
        b.push(down as u8);
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
        self.send(b);
    }
    fn on_close_request(&mut self) {
        // Ask the app to exit; it closes its window by returning, which drops
        // its channel end and lands as PEER_CLOSED in the next pump().
        self.send(OP_CLOSE_REQ.to_le_bytes().to_vec());
    }
}

/// Title bar + body padding the shell reserves around content (see
/// `Window::body`): TITLE_H + 6 top + 14 bottom.
const TITLE_H_PLUS: i32 = super::tokens::TITLE_H + 20;
