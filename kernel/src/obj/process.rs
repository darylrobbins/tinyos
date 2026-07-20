//! Process: an address space + handle table + (for now) one main thread.
//! Exit is a signal (`EXITED` + code) observable through a Process handle.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use spin::Mutex;

use crate::arch::paging::AddrSpace;

use super::channel::ChannelEnd;
use super::handle::HandleTable;
use super::memobj::MemObj;

const EXITED_BIT: u64 = 1 << 63;

/// Per-process physical-memory quota: MemObjs plus the image and stack.
/// Page-table frames (a handful) are deliberately not charged.
pub const MEM_QUOTA: usize = 64 * 1024 * 1024;

pub struct Process {
    pub id: u32,
    pub name: String,
    pub aspace: Arc<Mutex<AddrSpace>>,
    pub handles: Mutex<HandleTable>,
    pub main_thread: AtomicU32,
    /// MemObjs mapped into this process: keeps their frames alive at least
    /// as long as the address space that references them.
    pub mapped: Mutex<Vec<Arc<MemObj>>>,
    /// Kernel-held channel ends kept alive for the process's lifetime (the
    /// bootstrap main-channel end the kernel never speaks on again).
    pub keep: Mutex<Vec<Arc<ChannelEnd>>>,
    exit: AtomicU64, // EXITED_BIT | code (u32)
    /// Physical bytes charged against MEM_QUOTA.
    mem_charged: AtomicUsize,
}

static NEXT_PID: AtomicU32 = AtomicU32::new(1);
static PROCESSES: Mutex<Vec<Arc<Process>>> = Mutex::new(Vec::new());

impl Process {
    pub fn new(name: String, aspace: AddrSpace) -> Arc<Self> {
        let p = Arc::new(Self {
            id: NEXT_PID.fetch_add(1, Ordering::Relaxed),
            name,
            aspace: Arc::new(Mutex::new(aspace)),
            handles: Mutex::new(HandleTable::new()),
            main_thread: AtomicU32::new(0),
            mapped: Mutex::new(Vec::new()),
            keep: Mutex::new(Vec::new()),
            exit: AtomicU64::new(0),
            mem_charged: AtomicUsize::new(0),
        });
        PROCESSES.lock().push(p.clone());
        p
    }

    /// Charge `bytes` against the quota; false (nothing charged) if it
    /// would exceed MEM_QUOTA.
    pub fn try_charge(&self, bytes: usize) -> bool {
        let mut cur = self.mem_charged.load(Ordering::Relaxed);
        loop {
            let Some(next) = cur.checked_add(bytes).filter(|n| *n <= MEM_QUOTA) else {
                return false;
            };
            match self.mem_charged.compare_exchange_weak(
                cur,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(c) => cur = c,
            }
        }
    }

    /// Charge without a quota check — for kernel-controlled allocations at
    /// spawn (image, stack) that must not have a teardown failure path.
    pub fn charge(&self, bytes: usize) {
        self.mem_charged.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn uncharge(&self, bytes: usize) {
        self.mem_charged.fetch_sub(bytes, Ordering::Relaxed);
    }

    pub fn mem_charged(&self) -> usize {
        self.mem_charged.load(Ordering::Relaxed)
    }

    pub fn exited(&self) -> Option<i32> {
        let v = self.exit.load(Ordering::Acquire);
        (v & EXITED_BIT != 0).then_some(v as u32 as i32)
    }

    pub fn signals(&self) -> u32 {
        if self.exited().is_some() { super::SIG_EXITED } else { 0 }
    }

    /// Terminal state: record the code, close every handle (peers observe
    /// PEER_CLOSED), drop MemObj references, and deregister. The address
    /// space itself dies with the last Arc — after the exiting thread's
    /// final context switch away from it.
    pub fn set_exited(self: &Arc<Self>, code: i32) {
        self.exit
            .store(EXITED_BIT | code as u32 as u64, Ordering::Release);
        self.handles.lock().clear();
        self.mapped.lock().clear();
        self.keep.lock().clear();
        PROCESSES.lock().retain(|p| p.id != self.id);
        super::wake_objects();
    }

    #[allow(dead_code)] // process lookup by id, used by future proc-broker paths
    pub fn find(id: u32) -> Option<Arc<Process>> {
        PROCESSES.lock().iter().find(|p| p.id == id).cloned()
    }

    /// (pid, name, main thread, quota-charged bytes) per live process.
    pub fn snapshot() -> Vec<(u32, String, u32, usize)> {
        PROCESSES
            .lock()
            .iter()
            .map(|p| {
                (
                    p.id,
                    p.name.clone(),
                    p.main_thread.load(Ordering::Relaxed),
                    p.mem_charged(),
                )
            })
            .collect()
    }
}
