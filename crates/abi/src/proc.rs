//! Process-control protocol v0 (app <-> kernel over the PROC channel,
//! bootstrap grant TAG_PROC). Deliberately narrow: system stats, a
//! thread/process listing, and kill — the capabilities `ps`/`memstat`/
//! `kill` need, nothing more. Request/response like abi::fs.

// Requests.
pub const OP_SYSINFO: u32 = 1; // -> R_SYSINFO
pub const OP_PS: u32 = 2; // -> R_PS
pub const OP_KILL: u32 = 3; // {thread_id:u32} -> R_STATUS

// Replies.
pub const R_STATUS: u32 = 64; // {status:u32}
pub const R_SYSINFO: u32 = 65; // {status:u32, heap_used:u64, heap_free:u64,
                               //  pool_total:u64, pool_free:u64, uptime_us:u64}
pub const R_PS: u32 = 66; // {status:u32, nthreads:u32, per thread:
                          //  id:u32, cpu:u32, class:u32, state:u32,
                          //  namelen:u32, name:utf8;
                          //  nprocs:u32, per process: pid:u32, tid:u32,
                          //  mem:u64, namelen:u32, name:utf8}

// Status codes.
pub const PROC_OK: u32 = 0;
pub const PROC_NOT_FOUND: u32 = 1;
pub const PROC_DENIED: u32 = 2; // e.g. refusing to kill the UI thread
pub const PROC_INVALID: u32 = 3;

// Thread states in R_PS.
pub const STATE_READY: u32 = 0;
pub const STATE_RUNNING: u32 = 1;
pub const STATE_BLOCKED: u32 = 2;
pub const STATE_EXITED: u32 = 3;
