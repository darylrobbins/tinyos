use core::fmt::{self, Write};

use spin::Mutex;

use crate::arch::Serial;

static SERIAL: Mutex<Serial> = Mutex::new(Serial::new());

pub fn _print(args: fmt::Arguments) {
    SERIAL.lock().write_fmt(args).ok();
}

/// For the panic handler: the panicking context may hold the serial lock.
pub unsafe fn force_unlock() {
    unsafe { SERIAL.force_unlock() }
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => ($crate::logger::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! kprintln {
    () => ($crate::kprint!("\n"));
    ($($arg:tt)*) => ($crate::kprint!("{}\n", format_args!($($arg)*)));
}
