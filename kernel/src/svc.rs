//! Standing kernel services (FS, PROC) exposed as broker channels. Created at
//! boot, pumped by the ui_thread. In-kernel callers on that thread mint
//! directly (mint_fs/mint_proc — a broker round-trip would deadlock the pump);
//! userspace spawners are granted the broker channels (fs/proc_broker_handle).
//! See docs/superpowers/specs/2026-07-19-service-brokers-design.md.

use alloc::sync::Arc;

use spin::Mutex;

use crate::fs::server::FsServer;
use crate::obj::channel::{self, ChannelEnd};
use crate::obj::handle::{Handle, RIGHTS_ALL};
use crate::obj::procserver::ProcServer;
use crate::obj::Object;

static FS_SERVER: Mutex<Option<FsServer>> = Mutex::new(None);
static PROC_SERVER: Mutex<Option<ProcServer>> = Mutex::new(None);
static FS_BROKER_CLIENT: Mutex<Option<Arc<ChannelEnd>>> = Mutex::new(None);
static PROC_BROKER_CLIENT: Mutex<Option<Arc<ChannelEnd>>> = Mutex::new(None);
/// svcd's main-channel kernel end, parked for the supervisor's lifetime.
/// (aarch64 only — no userspace supervisor off aarch64.)
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
static SVCD_MAIN: Mutex<Option<Arc<ChannelEnd>>> = Mutex::new(None);

/// Create the FS/PROC brokers. Call once at boot, before any process spawns.
pub fn init() {
    let (fs_client, fs_server) = channel::create();
    let (proc_client, proc_server) = channel::create();
    *FS_SERVER.lock() = Some(FsServer::new(fs_server));
    *PROC_SERVER.lock() = Some(ProcServer::new(proc_server));
    *FS_BROKER_CLIENT.lock() = Some(fs_client);
    *PROC_BROKER_CLIENT.lock() = Some(proc_client);
}

/// Pump both servers; call once per ui_thread iteration.
pub fn pump() {
    if let Some(s) = FS_SERVER.lock().as_mut() {
        s.pump();
    }
    if let Some(s) = PROC_SERVER.lock().as_mut() {
        s.pump();
    }
}

/// Mint a fresh FS connection for an in-kernel spawner (direct, same-thread).
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))] // only boot_services (aarch64) mints today
pub fn mint_fs() -> Handle {
    FS_SERVER.lock().as_mut().expect("svc::init before spawn").mint()
}

/// Mint a fresh PROC connection for an in-kernel spawner.
#[allow(dead_code)] // in-kernel PROC minting is unused until a kernel spawner needs it
pub fn mint_proc() -> Handle {
    PROC_SERVER.lock().as_mut().expect("svc::init before spawn").mint()
}

/// A transferable handle to the FS broker client end — grant to a userspace
/// spawner so it can mint connections for its own children.
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))] // granted by boot_services (aarch64)
pub fn fs_broker_handle() -> Handle {
    let c = FS_BROKER_CLIENT.lock().as_ref().expect("svc::init").clone();
    Handle::new(Object::Channel(c), RIGHTS_ALL)
}

/// A transferable handle to the PROC broker client end.
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))] // granted by boot_services (aarch64)
pub fn proc_broker_handle() -> Handle {
    let c = PROC_BROKER_CLIENT.lock().as_ref().expect("svc::init").clone();
    Handle::new(Object::Channel(c), RIGHTS_ALL)
}

/// Boot-spawn the userspace service supervisor (`svcd`) from `/system/bin/svcd`,
/// granting it a full-root FS connection + the FS/PROC brokers. Its main-channel
/// end is parked in `SVCD_MAIN` for the process lifetime (dropping it would
/// signal `PEER_CLOSED` to svcd's handle 1). Call once from the ui_thread after
/// the scheduler + user paging are up. aarch64 only (userspace is aarch64-first).
#[cfg(target_arch = "aarch64")]
pub fn boot_services() {
    use abi::bootstrap::{TAG_FS, TAG_FS_BROKER, TAG_PROC_BROKER};
    let elf = match crate::fs::read("/", "/system/bin/svcd") {
        Ok(e) => e,
        Err(e) => {
            kprintln!("svcd: /system/bin/svcd: {e}");
            return;
        }
    };
    let grants = alloc::vec![
        (TAG_FS, mint_fs()),
        (TAG_FS_BROKER, fs_broker_handle()),
        (TAG_PROC_BROKER, proc_broker_handle()),
    ];
    match crate::obj::loader::spawn_with_grants(alloc::string::String::from("svcd"), &elf, &[], grants)
    {
        Ok((_proc, _tid, main_kern)) => *SVCD_MAIN.lock() = Some(main_kern),
        Err(e) => kprintln!("svcd: spawn failed: {}", e.msg()),
    }
}

/// No userspace supervisor off aarch64 (no EL0/ttbr1 machinery).
#[cfg(not(target_arch = "aarch64"))]
pub fn boot_services() {}
