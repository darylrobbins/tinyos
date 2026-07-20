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
pub mod broker;
pub mod channel;
pub mod console;
pub mod entry;
pub mod fs;
pub mod proc;
pub mod process;
mod font8x8;
pub mod gfx;
pub mod memobj;
pub mod monofont;
pub mod syscall;
pub mod textpad;
pub mod ui;
pub mod uifont;
pub mod wait;
pub mod window;

pub use console::{read_line, ConsoleEvent, LiveRegion, TextSurface};
pub use entry::Env;

/// Re-export for macros (`declare_caps!` references abi constants).
pub use abi;

/// ABI version stamp placed in the `.tinyos_abi` section; the loader checks
/// it before running the app. `declare_caps!` appends a caps blob right
/// after it (section `.tinyos_abi.caps`, ordered by link.ld).
#[used]
#[link_section = ".tinyos_abi"]
static ABI_VERSION: u32 = syscall::ABI_VERSION;

/// Backing for `declare_caps!`: magic + u32 len + token bytes, placed
/// immediately after the ABI stamp so the loader reads it at image base + 4.
/// The magic marks an explicit declaration; without it the loader cannot
/// tell an empty caps list from a legacy binary's zero padding.
#[repr(C)]
pub struct CapsBlob<const N: usize> {
    pub magic: u32,
    pub len: u32,
    pub bytes: [u8; N],
}

/// Declare the capabilities this app needs, newline-separated:
/// `console`, `window`, `proc`, `proc.kill` (advisory), `fs:self`
/// (a private data dir), `fs:/shared/<dir>`. Spawners intersect these with
/// their own policy. Declaring `b""` means "no capabilities at all";
/// apps that don't invoke the macro get the legacy default grants.
///
/// ```ignore
/// tinyos_app::declare_caps!(b"console\nwindow\nfs:self");
/// ```
#[macro_export]
macro_rules! declare_caps {
    ($caps:expr) => {
        #[used]
        #[link_section = ".tinyos_abi.caps"]
        static TINYOS_CAPS: $crate::CapsBlob<{ $caps.len() }> = $crate::CapsBlob {
            magic: $crate::abi::bootstrap::CAPS_MAGIC,
            len: $caps.len() as u32,
            bytes: *$caps,
        };
    };
}

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
