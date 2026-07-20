//! Process bootstrap: the record a new process reads from its main channel.
//!
//! Layout (all integers LE): u32 abi_version; u32 argc, then per arg
//! u32 len + UTF-8 bytes; u32 grant_count, then per grant u32 tag (the
//! granted handles ride the same message, in tag order).

/// The main channel is always handle 1 (the loader installs it first).
pub const MAIN_CHANNEL: u32 = 1;

/// Magic prefix of the caps blob in the `.tinyos_abi` stamp ("CAPS", LE).
/// Its presence distinguishes an explicit declaration — even an empty one,
/// which means "no capabilities" — from the zero padding after a legacy
/// stamp, which gets the compatibility default grants.
pub const CAPS_MAGIC: u32 = u32::from_le_bytes(*b"CAPS");

// Grant tags.
pub const TAG_CONSOLE: u32 = 1;
pub const TAG_SHELL: u32 = 2;
pub const TAG_FS: u32 = 3;
pub const TAG_PROC: u32 = 4;
/// Broker channels: a spawner forwards these so a child can mint its OWN
/// fresh FS/PROC connections rather than share the spawner's.
pub const TAG_FS_BROKER: u32 = 5;
pub const TAG_PROC_BROKER: u32 = 6;
