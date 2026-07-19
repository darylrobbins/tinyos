//! Window client: open a window on the shell, present a shared BGRA surface,
//! and receive input events. Protocol per the app-api spec (u32 opcode +
//! payload; the surface travels as a mapped MemObj handle).

use alloc::vec::Vec;

use crate::channel::Channel;
use crate::syscall::*;
use crate::wait::{WaitItem, wait_many};

use abi::window::{
    OP_BUTTON, OP_CHAR, OP_CLOSE_REQ, OP_KEY, OP_OPEN, OP_OPENED, OP_POINTER, OP_PRESENT,
};

/// An input event delivered by the shell.
pub enum Event {
    Char(char),
    Key { code: u16, down: bool },
    /// Pointer moved; window-body-local coords (may fall outside the body
    /// while the left button is held mid-drag).
    PointerMoved { x: i32, y: i32 },
    /// Left button edge; body-local coords.
    Button { down: bool, x: i32, y: i32 },
    CloseRequested,
}

/// A window backed by a shared surface the app draws into.
pub struct Window {
    ch: Channel,
    pub width: u32,
    pub height: u32,
    surface_va: u64,
}

impl Window {
    /// Open a `w`×`h` window titled `title` on the shell channel.
    pub fn open(shell: Channel, w: u32, h: u32, title: &str) -> Result<Window, u32> {
        // Shared BGRA surface: w*h*4 bytes, rounded to the map granularity.
        let size = (w as u64 * h as u64 * 4 + 0xFFF) & !0xFFF;
        let mem = syscall1(SYS_MEMOBJ_CREATE, size).ok()?;
        let dup = syscall2(SYS_HANDLE_DUP, mem, RIGHTS_ALL as u64).ok()? as u32;
        let surface_va = syscall3(SYS_MEMOBJ_MAP, mem, 0, size).ok()?;

        let mut msg = Vec::new();
        msg.extend_from_slice(&OP_OPEN.to_le_bytes());
        msg.extend_from_slice(&w.to_le_bytes());
        msg.extend_from_slice(&h.to_le_bytes());
        msg.extend_from_slice(&(title.len() as u32).to_le_bytes());
        msg.extend_from_slice(title.as_bytes());
        shell.send(&msg, &[dup])?;

        // Await OPENED.
        loop {
            let m = shell.recv()?;
            if m.bytes.len() >= 4 && u32::from_le_bytes(m.bytes[0..4].try_into().unwrap()) == OP_OPENED {
                break;
            }
        }
        Ok(Window { ch: shell, width: w, height: h, surface_va })
    }

    /// The BGRA pixel buffer (row-major, stride = width).
    pub fn pixels(&mut self) -> &mut [u32] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.surface_va as *mut u32,
                (self.width * self.height) as usize,
            )
        }
    }

    /// Copy a fully-drawn back buffer into the surface, then present. The
    /// shell only displays presented frames, so drawing into a back buffer
    /// and presenting with this keeps partial renders off screen.
    pub fn present_from(&mut self, back: &[u32]) {
        let px = self.pixels();
        let n = px.len().min(back.len());
        px[..n].copy_from_slice(&back[..n]);
        self.present();
    }

    /// Present the whole surface.
    pub fn present(&self) {
        let mut msg = Vec::new();
        msg.extend_from_slice(&OP_PRESENT.to_le_bytes());
        for v in [0u32, 0, self.width, self.height] {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        let _ = self.ch.send(&msg, &[]);
    }

    /// Drain pending input events (non-blocking).
    pub fn poll_events(&self, out: &mut Vec<Event>) {
        while let Ok(m) = self.ch.try_recv() {
            if m.bytes.len() < 4 {
                continue;
            }
            let op = u32::from_le_bytes(m.bytes[0..4].try_into().unwrap());
            match op {
                OP_CHAR if m.bytes.len() >= 8 => {
                    let c = u32::from_le_bytes(m.bytes[4..8].try_into().unwrap());
                    if let Some(c) = char::from_u32(c) {
                        out.push(Event::Char(c));
                    }
                }
                OP_KEY if m.bytes.len() >= 7 => {
                    let code = u16::from_le_bytes(m.bytes[4..6].try_into().unwrap());
                    out.push(Event::Key { code, down: m.bytes[6] != 0 });
                }
                OP_POINTER if m.bytes.len() >= 12 => {
                    let x = i32::from_le_bytes(m.bytes[4..8].try_into().unwrap());
                    let y = i32::from_le_bytes(m.bytes[8..12].try_into().unwrap());
                    out.push(Event::PointerMoved { x, y });
                }
                OP_BUTTON if m.bytes.len() >= 13 => {
                    let down = m.bytes[4] != 0;
                    let x = i32::from_le_bytes(m.bytes[5..9].try_into().unwrap());
                    let y = i32::from_le_bytes(m.bytes[9..13].try_into().unwrap());
                    out.push(Event::Button { down, x, y });
                }
                OP_CLOSE_REQ => out.push(Event::CloseRequested),
                _ => {}
            }
        }
    }

    /// Block until an event arrives or `deadline_us`.
    pub fn wait(&self, deadline_us: u64) {
        let mut it = [WaitItem { handle: self.ch.0, want: SIG_READABLE, observed: 0 }];
        let _ = wait_many(&mut it, deadline_us);
    }
}
