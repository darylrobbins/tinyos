//! File protocol v0 (app <-> filesystem service over the FS channel,
//! bootstrap grant TAG_FS). Request/response, one outstanding request per
//! channel (the SDK client blocks). All integers LE; paths are UTF-8,
//! resolved against the connection's base directory (the spawner's cwd).
//!
//! The service is in-kernel today; because clients only see this protocol,
//! it can re-host to a userspace fsd without any ABI change. Offset-based
//! I/O is in the protocol from day one so a cache can slot in later.

// Requests (u32 opcode + payload).
pub const OP_OPEN: u32 = 1; // {flags:u32, path:utf8}
pub const OP_CLOSE: u32 = 2; // {fd:u32}
pub const OP_READ: u32 = 3; // {fd:u32, offset:u64, len:u32}
pub const OP_WRITE: u32 = 4; // {fd:u32, offset:u64, data} (offset u64::MAX = append)
pub const OP_STAT: u32 = 5; // {path:utf8}
pub const OP_READDIR: u32 = 6; // {path:utf8}
pub const OP_MKDIR: u32 = 7; // {path:utf8}
pub const OP_REMOVE: u32 = 8; // {recursive:u32, path:utf8}
pub const OP_RENAME: u32 = 9; // {fromlen:u32, from:utf8, to:utf8}

// Replies.
pub const R_STATUS: u32 = 64; // {status:u32}
pub const R_OPEN: u32 = 65; // {status:u32, fd:u32}
pub const R_READ: u32 = 66; // {status:u32, data} (short read at EOF)
pub const R_STAT: u32 = 67; // {status:u32, kind:u32, size:u64}
pub const R_DIR: u32 = 68; // {status:u32, count:u32, then per entry:
                           //  kind:u32, size:u64, namelen:u32, name:utf8}

// OPEN flags.
pub const O_READ: u32 = 1;
pub const O_WRITE: u32 = 2;
pub const O_CREATE: u32 = 4;
pub const O_TRUNC: u32 = 8;

// Status codes.
pub const FS_OK: u32 = 0;
pub const FS_NOT_FOUND: u32 = 1;
pub const FS_IS_DIR: u32 = 2;
pub const FS_NOT_DIR: u32 = 3;
pub const FS_EXISTS: u32 = 4;
pub const FS_NO_SPACE: u32 = 5;
pub const FS_INVALID: u32 = 6;
pub const FS_IO: u32 = 7;
pub const FS_LIMIT: u32 = 8;
pub const FS_BAD_FD: u32 = 9;
pub const FS_DENIED: u32 = 10;

// Entry kinds.
pub const KIND_FILE: u32 = 0;
pub const KIND_DIR: u32 = 1;

/// Max bytes per READ/WRITE chunk (fits the 64 KiB channel message cap).
pub const MAX_IO: u32 = 32 * 1024;
/// Max open files per connection.
pub const MAX_FDS: usize = 16;
/// Append sentinel for OP_WRITE's offset.
pub const OFFSET_APPEND: u64 = u64::MAX;
