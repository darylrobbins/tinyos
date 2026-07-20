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
pub fn mint_fs() -> Handle {
    FS_SERVER.lock().as_mut().expect("svc::init before spawn").mint()
}

/// Mint a fresh PROC connection for an in-kernel spawner.
pub fn mint_proc() -> Handle {
    PROC_SERVER.lock().as_mut().expect("svc::init before spawn").mint()
}

/// A transferable handle to the FS broker client end — grant to a userspace
/// spawner so it can mint connections for its own children.
pub fn fs_broker_handle() -> Handle {
    let c = FS_BROKER_CLIENT.lock().as_ref().expect("svc::init").clone();
    Handle::new(Object::Channel(c), RIGHTS_ALL)
}

/// A transferable handle to the PROC broker client end.
pub fn proc_broker_handle() -> Handle {
    let c = PROC_BROKER_CLIENT.lock().as_ref().expect("svc::init").clone();
    Handle::new(Object::Channel(c), RIGHTS_ALL)
}
