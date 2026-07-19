//! `vicore` — a host-testable, `no_std` vi/vim-compatible editor engine.
//!
//! This crate contains the entire editor *model and behavior* — the buffer,
//! modal state machine, motions, operators, registers, undo/redo, search and
//! ex commands — with **no dependency on rendering or input hardware**. Input
//! arrives as semantic events ([`Editor::on_char`], [`Editor::on_special`],
//! [`Editor::on_ctrl`]); side effects that touch the outside world (file I/O,
//! quitting) are returned as [`Effect`]s for the host to perform.
//!
//! The kernel's `ViApp` is a thin adapter over this crate. Because the kernel
//! only builds for a bare-metal UEFI target, keeping the logic here lets it be
//! unit-tested on the host with `cargo test -p vicore`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod buffer;
pub mod editor;

pub use buffer::{Buffer, Pos};
pub use editor::{Editor, Effect, Mode, Special};
