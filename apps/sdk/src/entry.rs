//! Process entry glue: `_start` sets up the heap, reads the bootstrap record
//! from the main channel, and calls the app's `main(Env) -> i32`.
//!
//! The app links against this crate and provides `fn main(env: Env) -> i32`
//! via the `tinyos_app::app!` macro.

use alloc::string::String;
use alloc::vec::Vec;

use crate::channel::{Channel, Msg};
use crate::console::Console;
use crate::syscall::*;

/// Bootstrap grant tags (from the shared abi crate).
pub use abi::bootstrap::{TAG_CONSOLE, TAG_FS, TAG_SHELL};

/// Everything an app receives at startup.
pub struct Env {
    pub args: Vec<String>,
    pub console: Channel,
    pub shell: Channel,
}

use abi::bootstrap::MAIN_CHANNEL;

static mut CONSOLE: Option<Console> = None;

/// The process console, for the print!/println! macros.
pub fn console() -> Option<&'static mut Console> {
    // Single-threaded app; the loader sets this before main runs.
    unsafe { (&mut *core::ptr::addr_of_mut!(CONSOLE)).as_mut() }
}

fn parse_bootstrap(msg: &Msg) -> Env {
    let b = &msg.bytes;
    let mut off = 0usize;
    let u32at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());

    let _abi = u32at(b, off);
    off += 4;
    let argc = u32at(b, off) as usize;
    off += 4;
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        let len = u32at(b, off) as usize;
        off += 4;
        args.push(String::from_utf8_lossy(&b[off..off + len]).into_owned());
        off += len;
    }
    let grant_count = u32at(b, off) as usize;
    off += 4;
    let mut console = Channel(0);
    let mut shell = Channel(0);
    let mut fs = Channel(0);
    for i in 0..grant_count {
        let tag = u32at(b, off);
        off += 4;
        let handle = msg.handles.get(i).copied().unwrap_or(0);
        match tag {
            TAG_CONSOLE => console = Channel(handle),
            TAG_SHELL => shell = Channel(handle),
            TAG_FS => fs = Channel(handle),
            _ => {}
        }
    }
    if fs.0 != 0 {
        crate::fs::set_client(fs);
    }
    Env { args, console, shell }
}

/// Called by the `app!`-generated `_start`. Never returns.
pub fn run(main: fn(Env) -> i32) -> ! {
    crate::alloc_impl::init();
    let env = match Channel(MAIN_CHANNEL).recv() {
        Ok(msg) => parse_bootstrap(&msg),
        Err(_) => Env { args: Vec::new(), console: Channel(0), shell: Channel(0) },
    };
    unsafe {
        CONSOLE = Some(Console::new(env.console));
    }
    let code = main(env);
    exit(code);
}

pub fn exit(code: i32) -> ! {
    syscall1(SYS_PROCESS_EXIT, code as u32 as u64);
    // process_exit never returns; loop to satisfy the type.
    loop {
        core::hint::spin_loop();
    }
}

/// Define the app entry point. Usage: `tinyos_app::app!(main);`
#[macro_export]
macro_rules! app {
    ($main:path) => {
        #[no_mangle]
        pub extern "C" fn _start() -> ! {
            $crate::entry::run($main)
        }
    };
}
