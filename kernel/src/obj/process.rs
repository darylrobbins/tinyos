//! Process: an address space + handle table + (for now) one main thread.
//! Exit is a signal (`EXITED` + code) observable through a Process handle.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use spin::Mutex;

use crate::arch::paging::AddrSpace;

use super::handle::HandleTable;
use super::memobj::MemObj;

const EXITED_BIT: u64 = 1 << 63;

pub struct Process {
    pub id: u32,
    pub name: String,
    pub aspace: Arc<Mutex<AddrSpace>>,
    pub handles: Mutex<HandleTable>,
    pub main_thread: AtomicU32,
    /// MemObjs mapped into this process: keeps their frames alive at least
    /// as long as the address space that references them.
    pub mapped: Mutex<Vec<Arc<MemObj>>>,
    exit: AtomicU64, // EXITED_BIT | code (u32)
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
            exit: AtomicU64::new(0),
        });
        PROCESSES.lock().push(p.clone());
        p
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
        PROCESSES.lock().retain(|p| p.id != self.id);
        super::wake_objects();
    }

    pub fn find(id: u32) -> Option<Arc<Process>> {
        PROCESSES.lock().iter().find(|p| p.id == id).cloned()
    }

    pub fn snapshot() -> Vec<(u32, String, u32)> {
        PROCESSES
            .lock()
            .iter()
            .map(|p| (p.id, p.name.clone(), p.main_thread.load(Ordering::Relaxed)))
            .collect()
    }
}
