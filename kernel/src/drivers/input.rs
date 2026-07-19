//! virtio-input devices (keyboard + tablet) and event decoding.

use alloc::vec::Vec;

use super::pci::{self, BarAllocator};
use super::virtio::VirtioDevice;

pub const VIRTIO_ID_INPUT: u16 = 0x1052;

// evdev event types
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;

const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
/// QEMU tablet reports absolute coordinates in 0..=0x7fff.
pub const ABS_MAX: u32 = 0x7fff;

const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Key { code: u16, down: bool },
    PointerX(u32),
    PointerY(u32),
    Button { right: bool, down: bool },
    Frame,
}

pub struct Input {
    devices: Vec<VirtioDevice>,
}

impl Input {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    /// Claim one virtio-input device (called from `drivers::probe`).
    pub fn claim(&mut self, dev: &pci::PciDevice, alloc: &mut BarAllocator) {
        match VirtioDevice::init(dev, alloc, 8) {
            Some(v) => {
                kprintln!("tinyos: virtio-input ready (bdf {:#x})", dev.bdf);
                // Register the ISR byte so the IRQ handler can
                // deassert level-triggered INTx.
                let slot = &crate::arch::irq::INPUT_ISR_ADDRS[self.devices.len()];
                slot.store(v.isr_addr(), core::sync::atomic::Ordering::Relaxed);
                #[cfg(target_arch = "x86_64")]
                crate::arch::irq::register_input_gsi(
                    self.devices.len(),
                    dev.interrupt_line() as u32,
                );
                self.devices.push(v);
            }
            None => kprintln!("tinyos: virtio-input init FAILED (bdf {:#x})", dev.bdf),
        }
    }

    /// Drain all pending events from every input device.
    pub fn poll(&mut self, events: &mut Vec<Event>) {
        let mut buf = [0u8; 8];
        for dev in &mut self.devices {
            while dev.poll(&mut buf) {
                let ty = u16::from_le_bytes([buf[0], buf[1]]);
                let code = u16::from_le_bytes([buf[2], buf[3]]);
                let value = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
                let ev = match (ty, code) {
                    (EV_SYN, _) => Some(Event::Frame),
                    (EV_ABS, ABS_X) => Some(Event::PointerX(value)),
                    (EV_ABS, ABS_Y) => Some(Event::PointerY(value)),
                    (EV_KEY, BTN_LEFT) => Some(Event::Button {
                        right: false,
                        down: value != 0,
                    }),
                    (EV_KEY, BTN_RIGHT) => Some(Event::Button {
                        right: true,
                        down: value != 0,
                    }),
                    (EV_KEY, c) if c < 0x100 => Some(Event::Key {
                        code: c,
                        down: value != 0,
                    }),
                    _ => None,
                };
                if let Some(ev) = ev {
                    events.push(ev);
                }
            }
        }
    }
}

/// US-layout keycode to character translation.
/// `\x01` marks non-printing slots (esc, backspace, enter) so that real
/// printable characters — including '*' — are never filtered out.
pub fn keycode_to_char(code: u16, shift: bool) -> Option<char> {
    const PLAIN: &str = "\x01\x011234567890-=\x01\tqwertyuiop[]\x01\x00asdfghjkl;'`\x00\\zxcvbnm,./";
    const SHIFTED: &str = "\x01\x01!@#$%^&*()_+\x01\tQWERTYUIOP{}\x01\x00ASDFGHJKL:\"~\x00|ZXCVBNM<>?";
    let table = if shift { SHIFTED } else { PLAIN };
    match code {
        57 => Some(' '),
        28 => Some('\n'),
        _ => {
            let c = table.chars().nth(code as usize)?;
            (c != '\x01' && c != '\x00').then_some(c)
        }
    }
}

/// Names for non-printing keys the shell cares about.
pub mod keys {
    pub const ESC: u16 = 1;
    pub const BACKSPACE: u16 = 14;
    pub const ENTER: u16 = 28;
    pub const LSHIFT: u16 = 42;
    pub const RSHIFT: u16 = 54;
    pub const LCTRL: u16 = 29;
    pub const UP: u16 = 103;
    pub const DOWN: u16 = 108;
    pub const LEFT: u16 = 105;
    pub const RIGHT: u16 = 106;
}
