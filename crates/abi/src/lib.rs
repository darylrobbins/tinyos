//! tinyOS ABI: the single source of truth shared by the kernel, the app SDK,
//! and host tools. Constants and plain-data layouts only — no I/O, no alloc.
//!
//! Anything defined here is a contract. Numbers are stable once shipped; new
//! ones append. See `docs/superpowers/specs/2026-07-18-app-api-design.md` and
//! `docs/superpowers/specs/2026-07-19-terminal-and-crates-design.md`.

#![no_std]

pub mod bootstrap;
pub mod console;
pub mod fs;
pub mod keys;
pub mod proc;
pub mod syscall;
pub mod tokens;
pub mod window;
