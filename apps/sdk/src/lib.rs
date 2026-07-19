//! tinyos-app: the SDK third-party apps link against. A tiny, capability-
//! based, no_std runtime over the tinyOS syscall ABI (v0).
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! extern crate alloc;
//! use tinyos_app::{app, println, entry::Env};
//! fn main(env: Env) -> i32 { println!("hi {:?}", env.args); 0 }
//! app!(main);
//! ```

#![no_std]

extern crate alloc;

pub mod alloc_impl;
pub mod channel;
pub mod console;
pub mod entry;
mod font8x8;
pub mod gfx;
pub mod syscall;
pub mod ui;
pub mod uifont;
pub mod wait;
pub mod window;

pub use console::{read_line, ConsoleEvent, TextSurface};
pub use entry::Env;

/// ABI version stamp placed in the `.tinyos_abi` section; the loader checks
/// it before running the app.
#[used]
#[link_section = ".tinyos_abi"]
static ABI_VERSION: u32 = syscall::ABI_VERSION;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Best-effort: report to the kernel log, then exit 101 (Rust convention).
    use core::fmt::Write;
    let mut buf = LogWriter { buf: [0; 256], len: 0 };
    let _ = write!(buf, "panic: {info}");
    buf.flush();
    entry::exit(101)
}

/// Fixed-size stack buffer for the panic message → SYS_LOG (no heap needed).
struct LogWriter {
    buf: [u8; 256],
    len: usize,
}

impl core::fmt::Write for LogWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

impl LogWriter {
    fn flush(&self) {
        crate::syscall::syscall2(
            crate::syscall::SYS_LOG,
            self.buf.as_ptr() as u64,
            self.len as u64,
        );
    }
}
