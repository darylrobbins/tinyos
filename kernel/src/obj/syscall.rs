//! Syscall dispatch (ABI v0). x8 = number, x0-x5 args; returns
//! (status, value). Numbers are stable once shipped — append, never renumber.
//!
//! User pointers are validated against the caller's recorded mappings, then
//! dereferenced directly: the caller's TTBR1 is active on this CPU and PAN
//! is clear, so user memory is plainly addressable during its own syscall.

#![allow(dead_code)]

use alloc::sync::Arc;
use alloc::vec::Vec;

use super::channel::{self, MAX_MSG_BYTES, MAX_MSG_HANDLES, Message};
use super::handle::{
    Handle, RIGHT_MAP, RIGHT_READ, RIGHT_TRANSFER, RIGHT_WAIT, RIGHT_WRITE, RIGHTS_ALL,
};
use super::memobj::MemObj;
use super::process::Process;
use super::Object;
use crate::arch::paging::MapFlags;
use crate::mem::frames::FRAME_SIZE;

pub const ABI_VERSION: u64 = 0;

// Status codes (kept in sync with the design spec and the SDK).
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

const LOG_MAX: u64 = 4096;
const MAX_WAIT_ITEMS: u64 = 32;

type SysResult = Result<u64, u32>;

pub fn dispatch(sysno: u64, args: [u64; 6]) -> (u32, u64) {
    let r = match sysno {
        SYS_LOG => sys_log(args[0], args[1]),
        SYS_HANDLE_CLOSE => sys_handle_close(args[0]),
        SYS_HANDLE_DUP => sys_handle_dup(args[0], args[1]),
        SYS_CHANNEL_CREATE => sys_channel_create(args[0]),
        SYS_CHANNEL_SEND => sys_channel_send(args[0], args[1], args[2], args[3], args[4]),
        SYS_CHANNEL_RECV => sys_channel_recv(args[0], args[1], args[2], args[3], args[4], args[5]),
        SYS_WAIT_MANY => sys_wait_many(args[0], args[1], args[2]),
        SYS_MEMOBJ_CREATE => sys_memobj_create(args[0]),
        SYS_MEMOBJ_MAP => sys_memobj_map(args[0], args[1], args[2]),
        SYS_MEMOBJ_SIZE => sys_memobj_size(args[0]),
        SYS_PROCESS_EXIT => exit_current(args[0] as u32 as i32),
        SYS_CLOCK_UPTIME => Ok(crate::arch::timer::uptime_us()),
        SYS_ABI_VERSION => Ok(ABI_VERSION),
        _ => Err(ST_NOT_SUPPORTED),
    };
    match r {
        Ok(v) => (ST_OK, v),
        Err(st) => (st, 0),
    }
}

/// Terminal exit path for the calling user thread: record the process exit
/// (closing its handles, waking watchers), then leave the scheduler.
pub fn exit_current(code: i32) -> ! {
    // Disarm the EL0 preemption timer this thread armed on its last entry —
    // otherwise it stays pending and fires into the idle thread we switch to.
    #[cfg(target_arch = "aarch64")]
    crate::arch::timer::clear_timer();
    let me = crate::sched::current();
    kprintln!("app[{}]: exit({code})", me.id);
    if let Some(p) = &me.proc {
        p.set_exited(code);
    }
    crate::sched::exit()
}

fn cur_proc() -> Result<Arc<Process>, u32> {
    crate::sched::current().proc.clone().ok_or(ST_NOT_SUPPORTED)
}

// --- user memory access -----------------------------------------------------

fn user_buf_ok(va: u64, len: u64, write: bool) -> bool {
    let me = crate::sched::current();
    match &me.aspace {
        Some(a) => a.lock().user_buf_ok(va, len, write),
        None => false,
    }
}

fn copy_in(va: u64, len: u64) -> Result<Vec<u8>, u32> {
    if !user_buf_ok(va, len, false) {
        return Err(ST_INVALID_ARGS);
    }
    let mut v = alloc::vec![0u8; len as usize];
    unsafe { core::ptr::copy_nonoverlapping(va as *const u8, v.as_mut_ptr(), len as usize) };
    Ok(v)
}

fn copy_out(va: u64, bytes: &[u8]) -> Result<(), u32> {
    if !user_buf_ok(va, bytes.len() as u64, true) {
        return Err(ST_INVALID_ARGS);
    }
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), va as *mut u8, bytes.len()) };
    Ok(())
}

fn copy_out_u32s(va: u64, vals: &[u32]) -> Result<(), u32> {
    let mut bytes = Vec::with_capacity(vals.len() * 4);
    for v in vals {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    copy_out(va, &bytes)
}

// --- syscalls ---------------------------------------------------------------

fn sys_log(buf: u64, len: u64) -> SysResult {
    if len > LOG_MAX {
        return Err(ST_INVALID_ARGS);
    }
    let bytes = copy_in(buf, len)?;
    match core::str::from_utf8(&bytes) {
        Ok(s) => {
            let id = crate::sched::current_id();
            kprintln!("app[{id}]: {}", s.trim_end_matches('\n'));
            Ok(0)
        }
        Err(_) => Err(ST_INVALID_ARGS),
    }
}

fn sys_handle_close(hv: u64) -> SysResult {
    let p = cur_proc()?;
    p.handles.lock().take(hv as u32)?;
    Ok(0)
}

fn sys_handle_dup(hv: u64, mask: u64) -> SysResult {
    let p = cur_proc()?;
    let new = p.handles.lock().dup(hv as u32, mask as u32)?;
    Ok(new as u64)
}

fn sys_channel_create(out_ptr: u64) -> SysResult {
    let p = cur_proc()?;
    let (a, b) = channel::create();
    let mut t = p.handles.lock();
    let ha = t.insert(Handle::new(Object::Channel(a), RIGHTS_ALL))?;
    let hb = match t.insert(Handle::new(Object::Channel(b), RIGHTS_ALL)) {
        Ok(h) => h,
        Err(e) => {
            let _ = t.take(ha);
            return Err(e);
        }
    };
    drop(t);
    if let Err(e) = copy_out_u32s(out_ptr, &[ha, hb]) {
        let mut t = p.handles.lock();
        let _ = t.take(ha);
        let _ = t.take(hb);
        return Err(e);
    }
    Ok(0)
}

fn sys_channel_send(hv: u64, bytes: u64, blen: u64, hptr: u64, hcount: u64) -> SysResult {
    if blen as usize > MAX_MSG_BYTES || hcount as usize > MAX_MSG_HANDLES {
        return Err(ST_INVALID_ARGS);
    }
    let p = cur_proc()?;
    let ch = {
        let t = p.handles.lock();
        let h = t.get(hv as u32)?;
        if h.rights & RIGHT_WRITE == 0 {
            return Err(ST_ACCESS_DENIED);
        }
        match &h.object {
            Object::Channel(c) => c.clone(),
            _ => return Err(ST_WRONG_TYPE),
        }
    };
    let payload = copy_in(bytes, blen)?;
    let hvals_raw = copy_in(hptr, hcount * 4)?;
    let hvals: Vec<u32> = hvals_raw
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();

    // Move the handles out of the table (TRANSFER required); on a failed
    // send, put them back — their slots were just freed, reinsert can't fail.
    let mut moved = Vec::with_capacity(hvals.len());
    {
        let mut t = p.handles.lock();
        for &v in &hvals {
            match t.get(v) {
                Ok(h) if h.rights & RIGHT_TRANSFER == 0 => {
                    for (v, h) in hvals.iter().zip(moved.drain(..)) {
                        let _ = t.insert_back(*v, h);
                    }
                    return Err(ST_ACCESS_DENIED);
                }
                Ok(_) => moved.push(t.take(v).unwrap()),
                Err(e) => {
                    for (v, h) in hvals.iter().zip(moved.drain(..)) {
                        let _ = t.insert_back(*v, h);
                    }
                    return Err(e);
                }
            }
        }
    }
    match ch.send(Message { bytes: payload, handles: moved }) {
        Ok(()) => Ok(0),
        Err(e) => Err(e), // moved handles are gone with the failed message
    }
}

fn sys_channel_recv(hv: u64, buf: u64, cap: u64, hbuf: u64, hcap: u64, out_lens: u64) -> SysResult {
    let p = cur_proc()?;
    let ch = {
        let t = p.handles.lock();
        let h = t.get(hv as u32)?;
        if h.rights & RIGHT_READ == 0 {
            return Err(ST_ACCESS_DENIED);
        }
        match &h.object {
            Object::Channel(c) => c.clone(),
            _ => return Err(ST_WRONG_TYPE),
        }
    };
    // Size check against the queued head without consuming it.
    let (nbytes, nhandles) = match ch.peek() {
        Some(sz) => sz,
        None => return Err(if ch.signals() & super::SIG_PEER_CLOSED != 0 {
            ST_PEER_CLOSED
        } else {
            ST_SHOULD_WAIT
        }),
    };
    copy_out_u32s(out_lens, &[nbytes as u32, nhandles as u32])?;
    if nbytes as u64 > cap || nhandles as u64 > hcap {
        return Err(ST_BUFFER_TOO_SMALL);
    }
    let msg = ch.recv().map_err(|e| e)?;
    copy_out(buf, &msg.bytes)?;
    let mut hvals = Vec::with_capacity(msg.handles.len());
    {
        let mut t = p.handles.lock();
        for h in msg.handles {
            match t.insert(h) {
                Ok(v) => hvals.push(v),
                Err(e) => return Err(e), // remaining handles drop (closed)
            }
        }
    }
    copy_out_u32s(hbuf, &hvals)?;
    Ok(0)
}

fn sys_wait_many(items_ptr: u64, count: u64, deadline_us: u64) -> SysResult {
    if count > MAX_WAIT_ITEMS {
        return Err(ST_INVALID_ARGS);
    }
    if count == 0 {
        // Pure sleep until the deadline (or kill).
        let mut none: [(Object, u32, u32); 0] = [];
        let st = super::wait_many(&mut none, deadline_us);
        return if st == ST_OK { Ok(0) } else { Err(st) };
    }
    let p = cur_proc()?;
    let raw = copy_in(items_ptr, count * 12)?; // {handle, want, observed}: 3x u32
    let mut sets = Vec::with_capacity(count as usize);
    {
        let t = p.handles.lock();
        for c in raw.chunks_exact(12) {
            let hv = u32::from_le_bytes(c[0..4].try_into().unwrap());
            let want = u32::from_le_bytes(c[4..8].try_into().unwrap());
            let h = t.get(hv)?;
            if h.rights & RIGHT_WAIT == 0 {
                return Err(ST_ACCESS_DENIED);
            }
            sets.push((h.object.clone(), want, 0u32));
        }
    }
    let st = super::wait_many(&mut sets, deadline_us);
    // Write back observed signals regardless of status.
    let mut back = raw;
    for (i, (_, _, observed)) in sets.iter().enumerate() {
        back[i * 12 + 8..i * 12 + 12].copy_from_slice(&observed.to_le_bytes());
    }
    copy_out(items_ptr, &back)?;
    if st == ST_OK { Ok(0) } else { Err(st) }
}

fn sys_memobj_create(size: u64) -> SysResult {
    if size == 0 || size > 64 * 1024 * 1024 {
        return Err(ST_INVALID_ARGS);
    }
    let p = cur_proc()?;
    let m = MemObj::create(size as usize).ok_or(ST_NO_MEMORY)?;
    let hv = p.handles.lock().insert(Handle::new(Object::MemObj(m), RIGHTS_ALL))?;
    Ok(hv as u64)
}

fn sys_memobj_map(hv: u64, offset: u64, len: u64) -> SysResult {
    let p = cur_proc()?;
    let m = {
        let t = p.handles.lock();
        let h = t.get(hv as u32)?;
        if h.rights & RIGHT_MAP == 0 {
            return Err(ST_ACCESS_DENIED);
        }
        match &h.object {
            Object::MemObj(m) => m.clone(),
            _ => return Err(ST_WRONG_TYPE),
        }
    };
    // offset and len are both user-controlled: a wrapping `offset + len`
    // would let a huge offset pass the bounds check and then map arbitrary
    // physical memory via `m.pa() + offset`. Use checked arithmetic.
    let end = offset.checked_add(len);
    if offset % FRAME_SIZE as u64 != 0 || len == 0 || end.map_or(true, |e| e > m.size() as u64) {
        return Err(ST_INVALID_ARGS);
    }
    let va = {
        let mut a = p.aspace.lock();
        let va = a.alloc_va(len as usize);
        a.map(
            va,
            m.pa() + offset as usize,
            len as usize,
            MapFlags { write: true, exec: false },
            false,
        )
        .ok_or(ST_NO_MEMORY)?;
        va
    };
    p.mapped.lock().push(m);
    Ok(va)
}

fn sys_memobj_size(hv: u64) -> SysResult {
    let p = cur_proc()?;
    let t = p.handles.lock();
    match &t.get(hv as u32)?.object {
        Object::MemObj(m) => Ok(m.size() as u64),
        _ => Err(ST_WRONG_TYPE),
    }
}
