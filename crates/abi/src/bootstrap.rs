//! Process bootstrap: the record a new process reads from its main channel.
//!
//! Layout (all integers LE): u32 abi_version; u32 argc, then per arg
//! u32 len + UTF-8 bytes; u32 grant_count, then per grant u32 tag (the
//! granted handles ride the same message, in tag order).

/// The main channel is always handle 1 (the loader installs it first).
pub const MAIN_CHANNEL: u32 = 1;

// Grant tags.
pub const TAG_CONSOLE: u32 = 1;
pub const TAG_SHELL: u32 = 2;
pub const TAG_FS: u32 = 3;
pub const TAG_PROC: u32 = 4;
