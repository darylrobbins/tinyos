//! Meridian design tokens. The definitions live in the shared `abi` crate
//! (crates/abi/src/tokens.rs) so the app SDK sees identical values; this
//! module re-exports them plus kernel-local compatibility aliases.

pub use abi::tokens::*;

// Compatibility aliases used across the shell.
pub const SURFACE_HI: u32 = CARD2;
pub const TEXT: u32 = TX;
pub const TEXT_DIM: u32 = TX2;
pub const ACCENT: u32 = ACC;
pub const FIELD: u32 = BG;
pub const RADIUS: i32 = RADIUS_WIN;
pub const STATUS_H: i32 = 0; // no top bar in Meridian
