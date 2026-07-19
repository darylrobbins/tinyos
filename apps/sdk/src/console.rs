//! Console client: writes go to the terminal over the CONSOLE channel using
//! the console protocol (u32 opcode + payload).

use core::fmt::{self, Write};

use alloc::vec::Vec;

use crate::channel::Channel;

const OP_WRITE: u32 = 1;

pub struct Console {
    ch: Channel,
}

impl Console {
    pub fn new(ch: Channel) -> Self {
        Self { ch }
    }

    pub fn write_bytes(&self, s: &[u8]) {
        let mut msg = Vec::with_capacity(4 + s.len());
        msg.extend_from_slice(&OP_WRITE.to_le_bytes());
        msg.extend_from_slice(s);
        let _ = self.ch.send(&msg, &[]);
    }
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_bytes(s.as_bytes());
        Ok(())
    }
}

/// `println!`-style macros routed through the process console. Available
/// after `entry` stores the console handle.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        if let Some(c) = $crate::entry::console() {
            let _ = write!(c, $($arg)*);
        }
    }};
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        if let Some(c) = $crate::entry::console() {
            let _ = writeln!(c, $($arg)*);
        }
    }};
}
