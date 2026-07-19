//! Process-control service (abi::proc v0): one app's PROC channel, pumped
//! each frame by the terminal that spawned it. Same shape as fs::service —
//! in-kernel today, protocol-compatible with a userspace re-host.
//!
//! Authorization: KILL is (a) gated per service instance — spawners grant it
//! only to apps the user launched explicitly — and (b) limited to threads of
//! user processes; kernel threads are never killable over this protocol
//! (the trusted shell builtin still can). PS/SYSINFO expose global names and
//! sizes by design: that visibility is the tool's purpose and tinyOS has no
//! multi-user boundary — the capability gate is receiving the grant at all.

use alloc::sync::Arc;
use alloc::vec::Vec;

use abi::proc::*;

use crate::obj::channel::{ChannelEnd, Message};
use crate::sched;
use crate::sched::thread::{Class, State};

pub struct ProcService {
    ch: Arc<ChannelEnd>,
    can_kill: bool,
}

impl ProcService {
    pub fn new(ch: Arc<ChannelEnd>, can_kill: bool) -> Self {
        Self { ch, can_kill }
    }

    pub fn pump(&mut self) {
        while let Ok(msg) = self.ch.recv() {
            let reply = handle(&msg.bytes, self.can_kill);
            let _ = self.ch.send(Message { bytes: reply, handles: Vec::new() });
        }
    }
}

fn handle(b: &[u8], can_kill: bool) -> Vec<u8> {
    let op = b
        .get(0..4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()));
    match op {
        Some(OP_SYSINFO) => {
            let (used, free) = crate::mem::stats();
            let (pt, pf) = crate::mem::frames::pool_stats();
            let mut r = reply1(R_SYSINFO, PROC_OK);
            for v in [used as u64, free as u64, pt as u64, pf as u64] {
                r.extend_from_slice(&v.to_le_bytes());
            }
            r.extend_from_slice(&crate::arch::timer::uptime_us().to_le_bytes());
            r
        }
        Some(OP_PS) => {
            let threads = sched::snapshot();
            let mut r = reply1(R_PS, PROC_OK);
            r.extend_from_slice(&(threads.len() as u32).to_le_bytes());
            for t in threads {
                let state = match t.state {
                    State::Ready => STATE_READY,
                    State::Running => STATE_RUNNING,
                    State::Blocked => STATE_BLOCKED,
                    State::Exited => STATE_EXITED,
                };
                let class = match t.class {
                    Class::Idle => 0u32,
                    Class::Normal => 1,
                    Class::Interactive => 2,
                    Class::Realtime => 3,
                };
                for v in [t.id, t.cpu as u32, class, state, t.name.len() as u32] {
                    r.extend_from_slice(&v.to_le_bytes());
                }
                r.extend_from_slice(t.name.as_bytes());
            }
            let procs = crate::obj::process::Process::snapshot();
            r.extend_from_slice(&(procs.len() as u32).to_le_bytes());
            for (pid, name, tid, mem) in procs {
                for v in [pid, tid] {
                    r.extend_from_slice(&v.to_le_bytes());
                }
                r.extend_from_slice(&(mem as u64).to_le_bytes());
                r.extend_from_slice(&(name.len() as u32).to_le_bytes());
                r.extend_from_slice(name.as_bytes());
            }
            r
        }
        Some(OP_SHUTDOWN) | Some(OP_REBOOT) if can_kill => {
            // Privileged: only user-launched services (can_kill) may power
            // the machine. Sync the disk first; refuse to power off on a
            // failed sync (the one case that could lose the device cache).
            match crate::fs::sync() {
                Ok(()) => {
                    if op == Some(OP_REBOOT) {
                        kprintln!("tinyos: proc: filesystem synced, rebooting");
                        crate::arch::reboot()
                    } else {
                        kprintln!("tinyos: proc: filesystem synced, going down");
                        crate::arch::poweroff()
                    }
                }
                Err(_) => reply1(R_STATUS, PROC_INVALID),
            }
        }
        Some(OP_SHUTDOWN) | Some(OP_REBOOT) => reply1(R_STATUS, PROC_DENIED),
        Some(OP_SPIN) if can_kill => {
            let n = b
                .get(4..8)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .unwrap_or(1)
                .clamp(1, 16);
            let affinity = if sched::online_cpus() > 1 { 0b1110 } else { 0b0001 };
            for _ in 0..n {
                sched::spawn(
                    alloc::string::String::from("spin"),
                    Class::Normal,
                    affinity,
                    spin_worker,
                );
            }
            reply1(R_STATUS, PROC_OK)
        }
        Some(OP_SPIN) => reply1(R_STATUS, PROC_DENIED),
        Some(OP_KILL) => {
            let Some(id) = b
                .get(4..8)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            else {
                return reply1(R_STATUS, PROC_INVALID);
            };
            let is_user_thread = crate::obj::process::Process::snapshot()
                .iter()
                .any(|(_, _, tid, _)| *tid == id);
            if !can_kill || !is_user_thread || id == sched::ui_thread_id() {
                reply1(R_STATUS, PROC_DENIED)
            } else if sched::kill(id) {
                reply1(R_STATUS, PROC_OK)
            } else {
                reply1(R_STATUS, PROC_NOT_FOUND)
            }
        }
        _ => reply1(R_STATUS, PROC_INVALID),
    }
}

fn reply1(op: u32, st: u32) -> Vec<u8> {
    let mut v = op.to_le_bytes().to_vec();
    v.extend_from_slice(&st.to_le_bytes());
    v
}

/// Busy work in ~10 ms slices with a yield between, so cooperative
/// scheduling (and kill) always gets a look-in.
fn spin_worker() {
    loop {
        let t0 = crate::arch::timer::uptime_us();
        while crate::arch::timer::uptime_us() - t0 < 10_000 {
            core::hint::spin_loop();
        }
        sched::yield_now();
    }
}
