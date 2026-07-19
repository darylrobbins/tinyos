//! Process-control client (abi::proc v0) over the PROC channel grant.

use alloc::string::String;
use alloc::vec::Vec;

use abi::proc::*;

use crate::channel::{Channel, Msg};

static mut PROC: Option<Channel> = None;

pub(crate) fn set_client(ch: Channel) {
    unsafe { PROC = Some(ch) };
}

fn client() -> Option<Channel> {
    unsafe { (*core::ptr::addr_of!(PROC)) }
}

fn le_u32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap()))
}

fn le_u64(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8).map(|c| u64::from_le_bytes(c.try_into().unwrap()))
}

fn rpc(req: &[u8], expect: u32) -> Result<Msg, u32> {
    let ch = client().ok_or(PROC_INVALID)?;
    ch.send(req, &[]).map_err(|_| PROC_INVALID)?;
    let m = ch.recv().map_err(|_| PROC_INVALID)?;
    match le_u32(&m.bytes, 0) {
        Some(op) if op == expect => Ok(m),
        _ => Err(PROC_INVALID),
    }
}

pub struct SysInfo {
    pub heap_used: u64,
    pub heap_free: u64,
    pub pool_total: u64,
    pub pool_free: u64,
    pub uptime_us: u64,
}

pub struct ThreadRow {
    pub id: u32,
    pub cpu: u32,
    pub class: u32,
    pub state: u32,
    pub name: String,
}

pub struct ProcRow {
    pub pid: u32,
    pub tid: u32,
    pub mem: u64,
    pub name: String,
}

pub fn sysinfo() -> Result<SysInfo, u32> {
    let m = rpc(&OP_SYSINFO.to_le_bytes(), R_SYSINFO)?;
    let b = &m.bytes;
    Ok(SysInfo {
        heap_used: le_u64(b, 8).ok_or(PROC_INVALID)?,
        heap_free: le_u64(b, 16).ok_or(PROC_INVALID)?,
        pool_total: le_u64(b, 24).ok_or(PROC_INVALID)?,
        pool_free: le_u64(b, 32).ok_or(PROC_INVALID)?,
        uptime_us: le_u64(b, 40).ok_or(PROC_INVALID)?,
    })
}

pub fn ps() -> Result<(Vec<ThreadRow>, Vec<ProcRow>), u32> {
    let m = rpc(&OP_PS.to_le_bytes(), R_PS)?;
    let b = &m.bytes;
    let mut o = 8usize;
    let n = le_u32(b, o).ok_or(PROC_INVALID)? as usize;
    o += 4;
    let mut threads = Vec::with_capacity(n);
    for _ in 0..n {
        let id = le_u32(b, o).ok_or(PROC_INVALID)?;
        let cpu = le_u32(b, o + 4).ok_or(PROC_INVALID)?;
        let class = le_u32(b, o + 8).ok_or(PROC_INVALID)?;
        let state = le_u32(b, o + 12).ok_or(PROC_INVALID)?;
        let nlen = le_u32(b, o + 16).ok_or(PROC_INVALID)? as usize;
        let name = b.get(o + 20..o + 20 + nlen).ok_or(PROC_INVALID)?;
        threads.push(ThreadRow {
            id,
            cpu,
            class,
            state,
            name: String::from_utf8_lossy(name).into_owned(),
        });
        o += 20 + nlen;
    }
    let np = le_u32(b, o).ok_or(PROC_INVALID)? as usize;
    o += 4;
    let mut procs = Vec::with_capacity(np);
    for _ in 0..np {
        let pid = le_u32(b, o).ok_or(PROC_INVALID)?;
        let tid = le_u32(b, o + 4).ok_or(PROC_INVALID)?;
        let mem = le_u64(b, o + 8).ok_or(PROC_INVALID)?;
        let nlen = le_u32(b, o + 16).ok_or(PROC_INVALID)? as usize;
        let name = b.get(o + 20..o + 20 + nlen).ok_or(PROC_INVALID)?;
        procs.push(ProcRow {
            pid,
            tid,
            mem,
            name: String::from_utf8_lossy(name).into_owned(),
        });
        o += 20 + nlen;
    }
    Ok((threads, procs))
}

pub fn kill(thread_id: u32) -> Result<(), u32> {
    let mut r = OP_KILL.to_le_bytes().to_vec();
    r.extend_from_slice(&thread_id.to_le_bytes());
    let m = rpc(&r, R_STATUS)?;
    match le_u32(&m.bytes, 4) {
        Some(PROC_OK) => Ok(()),
        Some(st) => Err(st),
        None => Err(PROC_INVALID),
    }
}
