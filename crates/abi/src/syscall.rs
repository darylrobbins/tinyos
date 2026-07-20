//! Syscall ABI v0: numbers, status codes, signal bits, handle rights.
//!
//! aarch64: `svc #0`; x8 = syscall number, args in x0-x5; returns x0 = status,
//! x1 = value.

pub const ABI_VERSION: u32 = 0;

// Syscall numbers. 14 is reserved (thread_spawn).
pub const SYS_LOG: u64 = 0;
pub const SYS_HANDLE_CLOSE: u64 = 1;
pub const SYS_HANDLE_DUP: u64 = 2;
pub const SYS_CHANNEL_CREATE: u64 = 3;
pub const SYS_CHANNEL_SEND: u64 = 4;
pub const SYS_CHANNEL_RECV: u64 = 5;
pub const SYS_WAIT_MANY: u64 = 6;
pub const SYS_MEMOBJ_CREATE: u64 = 7;
pub const SYS_MEMOBJ_MAP: u64 = 8;
pub const SYS_MEMOBJ_SIZE: u64 = 9;
pub const SYS_PROCESS_EXIT: u64 = 10;
pub const SYS_CLOCK_UPTIME: u64 = 11;
pub const SYS_ABI_VERSION: u64 = 12;
/// Spawn a process: x0 = MemObj handle holding the ELF (READ), x1/x2 =
/// argv record ptr/len (u32 argc, then u32 len + utf8 per arg), x3/x4 =
/// grant array ptr/count ((tag u32, handle u32) pairs; handles need
/// TRANSFER and move), x5 = out *[u32;2] -> (process handle, parent end
/// of the child's main channel). Returns the child's thread id.
pub const SYS_PROCESS_SPAWN: u64 = 13;
/// Like SYS_PROCESS_SPAWN, but the kernel loads the app BY PATH (argv[0]),
/// attesting identity from the resolved /apps basename. flags bit 0 =
/// EXEC_REQUEST_WINDOW: mint a window under the attested identity (honored iff
/// the app's manifest declares `window`).
pub const SYS_PROCESS_EXEC: u64 = 16;

/// flags bit for SYS_PROCESS_EXEC: request a window for the child.
pub const EXEC_REQUEST_WINDOW: u64 = 1;
/// Unmap a memobj mapping by the vaddr `memobj_map` returned.
pub const SYS_MEMOBJ_UNMAP: u64 = 15;

/// Echo a string to serial as `[out] …` when the smoke-test mirror is on
/// (a kernel-side no-op otherwise). Lets the userspace terminal feed the
/// headless harness the same output the in-kernel terminal used to mirror.
pub const SYS_DEBUG_MIRROR: u64 = 17;

// Status codes.
pub const ST_OK: u32 = 0;
pub const ST_BAD_HANDLE: u32 = 1;
pub const ST_WRONG_TYPE: u32 = 2;
pub const ST_ACCESS_DENIED: u32 = 3;
pub const ST_INVALID_ARGS: u32 = 4;
pub const ST_PEER_CLOSED: u32 = 5;
pub const ST_SHOULD_WAIT: u32 = 6;
pub const ST_TIMED_OUT: u32 = 7;
pub const ST_NO_MEMORY: u32 = 8;
pub const ST_BUFFER_TOO_SMALL: u32 = 9;
pub const ST_LIMIT_EXCEEDED: u32 = 10;
pub const ST_NOT_SUPPORTED: u32 = 11;
pub const ST_KILLED: u32 = 12;

// Signal bits (wait_many, channel/process state).
pub const SIG_READABLE: u32 = 1;
pub const SIG_WRITABLE: u32 = 2;
pub const SIG_PEER_CLOSED: u32 = 4;
pub const SIG_EXITED: u32 = 8;

// Handle rights. `handle_dup` may only narrow.
pub const RIGHT_READ: u32 = 1;
pub const RIGHT_WRITE: u32 = 2;
pub const RIGHT_DUP: u32 = 4;
pub const RIGHT_TRANSFER: u32 = 8;
pub const RIGHT_MAP: u32 = 16;
pub const RIGHT_WAIT: u32 = 32;
pub const RIGHTS_ALL: u32 = 0x3F;
