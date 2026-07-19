//! Key codes delivered by window-protocol and console-protocol KEY events
//! (evdev-derived, as reported by virtio-input).

pub const ESC: u16 = 1;
pub const BACKSPACE: u16 = 14;
pub const ENTER: u16 = 28;
pub const LCTRL: u16 = 29;
pub const LSHIFT: u16 = 42;
pub const RSHIFT: u16 = 54;
pub const UP: u16 = 103;
pub const DOWN: u16 = 108;
pub const LEFT: u16 = 105;
pub const RIGHT: u16 = 106;
pub const KEY_S: u16 = 31;
pub const KEY_C: u16 = 46; // Ctrl+C interrupt
