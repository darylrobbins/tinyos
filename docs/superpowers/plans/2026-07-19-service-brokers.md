# Service Brokers + Per-Client Connections (SP0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the dup-shared FS/PROC channel (one service per process tree) with standing kernel FS/PROC servers that mint a fresh, isolated connection per client via a broker channel.

**Architecture:** Two kernel servers (`FsServer`, `ProcServer`), each a broker channel + a pool of live connections, pumped by the `ui_thread`. In-kernel spawners on that thread mint connections directly (`svc::mint_fs/mint_proc`); userspace spawners (`sh`) receive the broker channel and mint via `OP_CONNECT`. Leaf apps are unchanged — they still receive `TAG_FS`/`TAG_PROC` as a ready connection.

**Tech Stack:** Rust `no_std` (kernel: `aarch64-unknown-uefi` + `x86_64-unknown-uefi`; apps: `aarch64-unknown-none`), the `tinyos-abi` crate, `make` + QEMU + the `make smoke` harness.

## Global Constraints

- `no_std` everywhere; the `abi` crate is "constants and plain-data layouts only — no I/O, no alloc."
- Both kernel targets must keep compiling: `cargo check -p kernel --target aarch64-unknown-uefi` AND `--target x86_64-unknown-uefi`. Broker code is arch-neutral (pure `obj`/`fs` layer), so no `#[cfg]` needed, but always check both.
- Behavior-preserving: SP0 mints **full-root** FS (`jail="/"`, `base="/"`) and **`can_kill=true`** PROC connections — identical authority to today. No isolation *policy* changes.
- Moved handles that must survive broker→spawner→child carry `RIGHTS_ALL` (0x3F, includes `RIGHT_TRANSFER`); `dup` only narrows.
- The kernel terminal stays the boot default throughout; `make smoke` must stay green.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Spec: `docs/superpowers/specs/2026-07-19-service-brokers-design.md`.

---

## File structure

- `crates/abi/src/broker.rs` (create) — broker protocol constants.
- `crates/abi/src/lib.rs` (modify) — add `pub mod broker;`.
- `crates/abi/src/bootstrap.rs` (modify) — add `TAG_FS_BROKER`, `TAG_PROC_BROKER`.
- `apps/sdk/src/broker.rs` (create) — `connect(broker) -> Channel`.
- `apps/sdk/src/lib.rs` (modify) — add `pub mod broker;`.
- `apps/sdk/src/entry.rs` (modify) — `Env` broker fields + bootstrap parse.
- `kernel/src/fs/service.rs` (modify) — `FsService::is_open()`.
- `kernel/src/obj/procsrv.rs` (modify) — `ProcService::is_open()`.
- `kernel/src/fs/server.rs` (create) — `FsServer`.
- `kernel/src/fs/mod.rs` (modify) — `pub mod server;`.
- `kernel/src/obj/procserver.rs` (create) — `ProcServer`.
- `kernel/src/obj/mod.rs` (modify) — `pub mod procserver;`.
- `kernel/src/svc.rs` (create) — server globals, `init`, `pump`, `mint_*`, `*_broker_handle`.
- `kernel/src/main.rs` (modify) — `mod svc;`, `svc::init()` at boot, `svc::pump()` in `ui_thread_main`.
- `kernel/src/term/mod.rs` (modify) — rewire `spawn_app`; drop `RunningApp::{fs_srv,proc_srv}`; drop their pumps.
- `apps/shell/src/main.rs` (modify) — `child_grants()` mints per child + forwards brokers.
- `tools/smoke/smoke.py` (modify) — regression step.

---

### Task 1: ABI — broker protocol + tags

**Files:**
- Create: `crates/abi/src/broker.rs`
- Modify: `crates/abi/src/lib.rs:17` (module list), `crates/abi/src/bootstrap.rs:20` (after `TAG_PROC`)

**Interfaces:**
- Produces: `abi::broker::{OP_CONNECT, R_CONNECTED, BROKER_OK, BROKER_NOMEM}` (all `u32`); `abi::bootstrap::{TAG_FS_BROKER, TAG_PROC_BROKER}` (`u32` = 5, 6).

- [ ] **Step 1: Create the broker protocol module**

`crates/abi/src/broker.rs`:
```rust
//! Service broker protocol v0. A client sends OP_CONNECT (bytes = [OP_CONNECT])
//! on a broker channel; the server replies R_CONNECTED{status:u32}, and on
//! BROKER_OK the new connection's client end rides as the reply's single moved
//! handle. Identical whether the server is in-kernel (SP0) or a userspace fsd
//! later. See docs/superpowers/specs/2026-07-19-service-brokers-design.md.

pub const OP_CONNECT: u32 = 1;
pub const R_CONNECTED: u32 = 2;

pub const BROKER_OK: u32 = 0;
pub const BROKER_NOMEM: u32 = 1;
```

- [ ] **Step 2: Register the module**

`crates/abi/src/lib.rs` — add after `pub mod bootstrap;` (line 10):
```rust
pub mod broker;
```

- [ ] **Step 3: Add the bootstrap tags**

`crates/abi/src/bootstrap.rs` — add after line 20 (`pub const TAG_PROC: u32 = 4;`):
```rust
/// Broker channels: a spawner forwards these so a child can mint its OWN
/// fresh FS/PROC connections rather than share the spawner's.
pub const TAG_FS_BROKER: u32 = 5;
pub const TAG_PROC_BROKER: u32 = 6;
```

- [ ] **Step 4: Build the abi crate (host target — it's `no_std` lib, `cargo build -p` checks it)**

Run: `cargo build -p tinyos-abi`
Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**
```bash
git add crates/abi/src/broker.rs crates/abi/src/lib.rs crates/abi/src/bootstrap.rs
git commit -m "abi: broker protocol + FS/PROC broker bootstrap tags

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: SDK — broker client + Env broker fields

**Files:**
- Create: `apps/sdk/src/broker.rs`
- Modify: `apps/sdk/src/lib.rs:18` (module list), `apps/sdk/src/entry.rs` (Env struct + parse)

**Interfaces:**
- Consumes: `abi::broker::{OP_CONNECT, R_CONNECTED, BROKER_OK}` (Task 1); `Channel::{send,recv}` (`apps/sdk/src/channel.rs`).
- Produces: `tinyos_app::broker::connect(broker: Channel) -> Result<Channel, u32>`; `Env.fs_broker: Channel`, `Env.proc_broker: Channel`.

- [ ] **Step 1: Create the SDK broker client**

`apps/sdk/src/broker.rs`:
```rust
//! Broker client: request a fresh, private service connection from a broker
//! channel the spawner granted us. Blocks for the reply.

use crate::channel::Channel;
use abi::broker::{BROKER_OK, OP_CONNECT, R_CONNECTED};

fn le(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap()))
}

/// Malformed / handle-less broker reply (distinct from any BROKER_* status).
pub const ERR_BROKER_REPLY: u32 = u32::MAX;

/// Ask `broker` for a new connection. Returns the connection's client end.
pub fn connect(broker: Channel) -> Result<Channel, u32> {
    broker.send(&OP_CONNECT.to_le_bytes(), &[])?;
    let msg = broker.recv()?;
    if le(&msg.bytes, 0) != Some(R_CONNECTED) {
        return Err(ERR_BROKER_REPLY);
    }
    match le(&msg.bytes, 4) {
        Some(BROKER_OK) => msg.handles.first().copied().map(Channel).ok_or(ERR_BROKER_REPLY),
        Some(e) => Err(e),
        None => Err(ERR_BROKER_REPLY),
    }
}
```

- [ ] **Step 2: Register the module**

`apps/sdk/src/lib.rs` — add after `pub mod channel;` (line 18):
```rust
pub mod broker;
```

- [ ] **Step 3: Extend `Env` and the bootstrap parser**

`apps/sdk/src/entry.rs` — extend the tag re-export (line 15):
```rust
pub use abi::bootstrap::{TAG_CONSOLE, TAG_FS, TAG_FS_BROKER, TAG_PROC, TAG_PROC_BROKER, TAG_SHELL};
```
Add two fields to `Env` (after `pub proc: Channel,`, line 23):
```rust
    pub fs_broker: Channel,
    pub proc_broker: Channel,
```
In `parse_bootstrap`, add locals next to the others (after `let mut proc = Channel(0);`, line 57):
```rust
    let mut fs_broker = Channel(0);
    let mut proc_broker = Channel(0);
```
Add match arms in the grant loop (after `TAG_PROC => proc = Channel(handle),`, line 66):
```rust
            TAG_FS_BROKER => fs_broker = Channel(handle),
            TAG_PROC_BROKER => proc_broker = Channel(handle),
```
Update the `Env { ... }` return (line 76):
```rust
    Env { args, console, shell, fs, proc, fs_broker, proc_broker }
```
And the error-branch `Env { ... }` in `run` (lines 84-90) — add the two fields:
```rust
        Err(_) => Env {
            args: Vec::new(),
            console: Channel(0),
            shell: Channel(0),
            fs: Channel(0),
            proc: Channel(0),
            fs_broker: Channel(0),
            proc_broker: Channel(0),
        },
```

- [ ] **Step 4: Build the apps workspace (compiles the SDK + every app against the new `Env`)**

Run: `cd apps && cargo build --release`
Expected: `Finished`. Every app recompiles unchanged (they ignore the new fields).

- [ ] **Step 5: Commit**
```bash
git add apps/sdk/src/broker.rs apps/sdk/src/lib.rs apps/sdk/src/entry.rs
git commit -m "sdk: broker::connect + Env fs_broker/proc_broker fields

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Kernel — `is_open()` helpers for the connection pools

**Files:**
- Modify: `kernel/src/fs/service.rs` (add method to `impl FsService`), `kernel/src/obj/procsrv.rs` (add method + import)

**Interfaces:**
- Produces: `FsService::is_open(&self) -> bool`, `ProcService::is_open(&self) -> bool` (true while the client peer is still connected).

- [ ] **Step 1: Add `FsService::is_open`**

`kernel/src/fs/service.rs` — inside `impl FsService`, right after `pub fn new(...)` (line 83). `SIG_PEER_CLOSED` is already imported at line 16:
```rust
    /// True while the client end is still open. Used by FsServer to reap
    /// connections whose app has exited (its client handle was dropped).
    pub fn is_open(&self) -> bool {
        self.ch.signals() & SIG_PEER_CLOSED == 0
    }
```

- [ ] **Step 2: Add `ProcService::is_open` (+ import)**

`kernel/src/obj/procsrv.rs` — add the signal import near the top imports (after `use crate::obj::channel::{ChannelEnd, Message};`):
```rust
use crate::obj::SIG_PEER_CLOSED;
```
Inside `impl ProcService`, after `pub fn new(...)`:
```rust
    /// True while the client end is still open (see FsService::is_open).
    pub fn is_open(&self) -> bool {
        self.ch.signals() & SIG_PEER_CLOSED == 0
    }
```

- [ ] **Step 3: Check both kernel targets compile**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`. (Dead-code warning on the unused methods is fine — Tasks 4/5 consume them.)

- [ ] **Step 4: Commit**
```bash
git add kernel/src/fs/service.rs kernel/src/obj/procsrv.rs
git commit -m "kernel: FsService/ProcService is_open() for connection reaping

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Kernel — `FsServer`

**Files:**
- Create: `kernel/src/fs/server.rs`
- Modify: `kernel/src/fs/mod.rs:9` (add `pub mod server;`)

**Interfaces:**
- Consumes: `FsService::{new, pump, is_open}` (Task 3); `channel::create`, `Message`, `ChannelEnd` (`kernel/src/obj/channel.rs`); `Handle::new`, `RIGHTS_ALL` (`kernel/src/obj/handle.rs`); `Object::Channel` (`kernel/src/obj/mod.rs`); `abi::broker::*` (Task 1).
- Produces: `FsServer::{new(Arc<ChannelEnd>) -> Self, mint(&mut self) -> Handle, pump(&mut self)}`.

- [ ] **Step 1: Create `FsServer`**

`kernel/src/fs/server.rs`:
```rust
//! Standing FS server: a broker channel that mints a fresh, isolated FsService
//! connection per OP_CONNECT, plus the pool of live connections it pumps. The
//! same broker protocol whether served here or by a userspace fsd later.
//! See docs/superpowers/specs/2026-07-19-service-brokers-design.md.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use abi::broker::{BROKER_OK, OP_CONNECT, R_CONNECTED};

use crate::fs::service::FsService;
use crate::obj::channel::{self, ChannelEnd, Message};
use crate::obj::handle::{Handle, RIGHTS_ALL};
use crate::obj::Object;

pub struct FsServer {
    broker: Arc<ChannelEnd>,
    conns: Vec<FsService>,
}

impl FsServer {
    pub fn new(broker: Arc<ChannelEnd>) -> Self {
        Self { broker, conns: Vec::new() }
    }

    /// Mint one fresh, full-root connection; pool the server end and return the
    /// CLIENT end as a transferable handle. The single connection-creation path
    /// shared by direct in-kernel callers and the broker below.
    pub fn mint(&mut self) -> Handle {
        let (client, server) = channel::create();
        self.conns
            .push(FsService::new(server, String::from("/"), String::from("/")));
        Handle::new(Object::Channel(client), RIGHTS_ALL)
    }

    /// Serve queued OP_CONNECTs, then pump every live connection, reaping any
    /// whose client end has closed.
    pub fn pump(&mut self) {
        while let Ok(msg) = self.broker.recv() {
            let op = msg.bytes.get(0..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            if op == Some(OP_CONNECT) {
                let h = self.mint();
                let mut reply = R_CONNECTED.to_le_bytes().to_vec();
                reply.extend_from_slice(&BROKER_OK.to_le_bytes());
                let _ = self.broker.send(Message { bytes: reply, handles: vec![h] });
            }
        }
        self.conns.retain_mut(|c| {
            c.pump();
            c.is_open()
        });
    }
}
```

- [ ] **Step 2: Register the module**

`kernel/src/fs/mod.rs` — change line 9 (`pub mod service;`) to also declare:
```rust
pub mod server;
pub mod service;
```

- [ ] **Step 3: Check both kernel targets compile**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished` (dead-code warning on `FsServer` until Task 6 — fine).

- [ ] **Step 4: Commit**
```bash
git add kernel/src/fs/server.rs kernel/src/fs/mod.rs
git commit -m "kernel: FsServer — broker + per-client FsService pool

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Kernel — `ProcServer`

**Files:**
- Create: `kernel/src/obj/procserver.rs`
- Modify: `kernel/src/obj/mod.rs:10` (add `pub mod procserver;`)

**Interfaces:**
- Consumes: `ProcService::{new, pump, is_open}` (Task 3); same channel/handle/object primitives as Task 4.
- Produces: `ProcServer::{new(Arc<ChannelEnd>) -> Self, mint(&mut self) -> Handle, pump(&mut self)}`.

- [ ] **Step 1: Create `ProcServer`**

`kernel/src/obj/procserver.rs`:
```rust
//! Standing PROC server: mirror of FsServer for the process-control protocol.
//! Mints a fresh ProcService connection per OP_CONNECT. SP0 mints every
//! connection with can_kill=true (same authority as today); per-connection
//! privilege is a later policy layer.

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use abi::broker::{BROKER_OK, OP_CONNECT, R_CONNECTED};

use crate::obj::channel::{self, ChannelEnd, Message};
use crate::obj::handle::{Handle, RIGHTS_ALL};
use crate::obj::procsrv::ProcService;
use crate::obj::Object;

pub struct ProcServer {
    broker: Arc<ChannelEnd>,
    conns: Vec<ProcService>,
}

impl ProcServer {
    pub fn new(broker: Arc<ChannelEnd>) -> Self {
        Self { broker, conns: Vec::new() }
    }

    pub fn mint(&mut self) -> Handle {
        let (client, server) = channel::create();
        self.conns.push(ProcService::new(server, true));
        Handle::new(Object::Channel(client), RIGHTS_ALL)
    }

    pub fn pump(&mut self) {
        while let Ok(msg) = self.broker.recv() {
            let op = msg.bytes.get(0..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            if op == Some(OP_CONNECT) {
                let h = self.mint();
                let mut reply = R_CONNECTED.to_le_bytes().to_vec();
                reply.extend_from_slice(&BROKER_OK.to_le_bytes());
                let _ = self.broker.send(Message { bytes: reply, handles: vec![h] });
            }
        }
        self.conns.retain_mut(|c| {
            c.pump();
            c.is_open()
        });
    }
}
```

- [ ] **Step 2: Register the module**

`kernel/src/obj/mod.rs` — add after `pub mod procsrv;` (line 10):
```rust
pub mod procserver;
```

- [ ] **Step 3: Check both kernel targets compile**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`.

- [ ] **Step 4: Commit**
```bash
git add kernel/src/obj/procserver.rs kernel/src/obj/mod.rs
git commit -m "kernel: ProcServer — broker + per-client ProcService pool

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: Kernel — `svc` module + boot wiring + `ui_thread` pump

**Files:**
- Create: `kernel/src/svc.rs`
- Modify: `kernel/src/main.rs` (`mod svc;` at line 17-area; `svc::init()` after `fs::init`; `svc::pump()` in `ui_thread_main`)

**Interfaces:**
- Consumes: `FsServer` (Task 4), `ProcServer` (Task 5), `channel::create`, `Handle::new`, `RIGHTS_ALL`, `Object::Channel`.
- Produces: `svc::{init(), pump(), mint_fs() -> Handle, mint_proc() -> Handle, fs_broker_handle() -> Handle, proc_broker_handle() -> Handle}`.

- [ ] **Step 1: Create the `svc` module**

`kernel/src/svc.rs`:
```rust
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
```

- [ ] **Step 2: Declare the module**

`kernel/src/main.rs` — add to the module list (alphabetical, after `mod sched;` / before `mod term;`, near line 16-17):
```rust
mod svc;
```

- [ ] **Step 3: Initialize the brokers at boot**

`kernel/src/main.rs` — in `main()`, after `fs::init(blk);` (line 152) and before `arch::irq::init();`:
```rust
    // Standing FS/PROC broker servers (pumped by the ui_thread below).
    svc::init();
```

- [ ] **Step 4: Pump the servers each frame**

`kernel/src/main.rs` — in `ui_thread_main`'s loop, after `shell.pump_externals();` (line 187):
```rust
        crate::svc::pump();
```

- [ ] **Step 5: Check both kernel targets compile**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`. Servers now exist and are pumped, but nothing mints yet.

- [ ] **Step 6: Commit**
```bash
git add kernel/src/svc.rs kernel/src/main.rs
git commit -m "kernel: svc module — FS/PROC broker servers, boot init + ui_thread pump

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: Kernel — rewire `term::spawn_app` to mint + grant brokers

**Files:**
- Modify: `kernel/src/term/mod.rs` — `spawn_app` (lines 680-724); `RunningApp` struct (lines 48-56, remove `fs_srv`/`proc_srv`); `pump` (line ~801, remove the two service pumps); check `pump_bg` for the same references.

**Interfaces:**
- Consumes: `svc::{mint_fs, mint_proc, fs_broker_handle, proc_broker_handle}` (Task 6); `loader::spawn_with_grants` (`kernel/src/obj/loader.rs:289`); `channel::create`; `Handle::new`, `RIGHTS_ALL`; `Object::Channel`.
- Produces: (internal) each terminal-spawned app now has its own broker-minted FS/PROC connection and holds `TAG_FS_BROKER`/`TAG_PROC_BROKER`.

- [ ] **Step 1: Remove the per-child service fields from `RunningApp`**

`kernel/src/term/mod.rs` — delete these four lines from `struct RunningApp` (lines 50-53):
```rust
    /// File-protocol server for this app's FS channel.
    fs_srv: crate::fs::service::FsService,
    /// Process-control server for this app's PROC channel.
    proc_srv: crate::obj::procsrv::ProcService,
```

- [ ] **Step 2: Rewrite `spawn_app`**

`kernel/src/term/mod.rs` — replace the body of `spawn_app` (lines 681-724) with:
```rust
    fn spawn_app(&mut self, name: &str, argv: &[String], background: bool) -> Result<u32, String> {
        use crate::obj::channel::create;
        use crate::obj::handle::{Handle, RIGHTS_ALL};
        use crate::obj::Object;
        use abi::bootstrap::{
            TAG_CONSOLE, TAG_FS, TAG_FS_BROKER, TAG_PROC, TAG_PROC_BROKER, TAG_SHELL,
        };

        let elf = crate::fs::read("/", &format!("/apps/{name}")).map_err(|e| format!("{e}"))?;

        // Console + shell channels: this terminal keeps the kernel ends.
        let (console_app, console_kern) = create();
        let (shell_app, shell_kern) = create();

        // FS/PROC: a fresh isolated connection from the standing servers, plus
        // the broker channels so the child can mint for ITS own children.
        let grants: alloc::vec::Vec<(u32, Handle)> = alloc::vec![
            (TAG_CONSOLE, Handle::new(Object::Channel(console_app), RIGHTS_ALL)),
            (TAG_SHELL, Handle::new(Object::Channel(shell_app), RIGHTS_ALL)),
            (TAG_FS, crate::svc::mint_fs()),
            (TAG_PROC, crate::svc::mint_proc()),
            (TAG_FS_BROKER, crate::svc::fs_broker_handle()),
            (TAG_PROC_BROKER, crate::svc::proc_broker_handle()),
        ];

        let (process, tid, _main_kern) = crate::obj::loader::spawn_with_grants(
            name.to_string(),
            &elf,
            argv,
            grants,
        )
        .map_err(|e| e.msg())?;

        // Hand the window channel to the compositor.
        crate::ui::shell::extern_app::register(shell_kern, name.to_string());

        let job = RunningApp {
            name: name.to_string(),
            process,
            thread_id: tid,
            console: console_kern,
            partial: String::new(),
            partial_color: FG,
            prompt_spans: Vec::new(),
            input_mode: abi::console::INPUT_MODE_LINES,
            sent_size: (0, 0), // forces the initial OP_RESIZE
            foreground_tid: 0,
            surface: None,
            live: None,
        };
        if background {
            self.bg_jobs.push(job);
        } else {
            self.running = Some(job);
            self.view.set_prompt(Vec::new()); // the app owns the prompt now
        }
        Ok(tid)
    }
```

- [ ] **Step 3: Remove the per-child service pumps**

`kernel/src/term/mod.rs` — in `pump` (around line 801), delete:
```rust
        app.fs_srv.pump();
        app.proc_srv.pump();
```
Then search the file for any remaining references and remove them:

Run: `grep -n "fs_srv\|proc_srv" kernel/src/term/mod.rs`
Expected AFTER edits: no matches. (If `pump_bg` references them, delete those lines too — background jobs' connections are now served by `svc::pump`.)

- [ ] **Step 4: Check both kernel targets compile**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`. If `loader::spawn` / `GrantSet` are now unused, silence with `#[allow(dead_code)]` on them in `kernel/src/obj/loader.rs` rather than deleting (they may return in SP1).

- [ ] **Step 5: Commit**
```bash
git add kernel/src/term/mod.rs kernel/src/obj/loader.rs
git commit -m "term: spawn children via broker-minted FS/PROC connections

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: sh — mint per-child connections instead of dup-sharing

**Files:**
- Modify: `apps/shell/src/main.rs` — imports (line 15) and `child_grants()` (lines 78-93)

**Interfaces:**
- Consumes: `tinyos_app::broker::connect` (Task 2); `Env.fs_broker`, `Env.proc_broker` (Task 2); `abi::bootstrap::{TAG_FS_BROKER, TAG_PROC_BROKER}` (Task 1).

- [ ] **Step 1: Import the new tags**

`apps/shell/src/main.rs` — line 15, extend the import:
```rust
use abi::bootstrap::{TAG_CONSOLE, TAG_FS, TAG_FS_BROKER, TAG_PROC, TAG_PROC_BROKER, TAG_SHELL};
```

- [ ] **Step 2: Rewrite `child_grants`**

`apps/shell/src/main.rs` — replace `child_grants` (lines 78-93):
```rust
    /// Capabilities a child inherits. Console + shell are shared (dup); FS and
    /// PROC are a FRESH private connection minted per child from the broker, so
    /// siblings and background jobs never share a request/reply channel. The
    /// brokers themselves are forwarded so the child can mint for its children.
    fn child_grants(&self) -> Vec<(u32, u32)> {
        let mut g = Vec::new();
        for (tag, ch) in [(TAG_CONSOLE, self.env.console.0), (TAG_SHELL, self.env.shell.0)] {
            if ch != 0 {
                if let Ok(h) = syscall2(SYS_HANDLE_DUP, ch as u64, RIGHTS_ALL as u64).ok() {
                    g.push((tag, h as u32));
                }
            }
        }
        if self.env.fs_broker.0 != 0 {
            if let Ok(c) = tinyos_app::broker::connect(self.env.fs_broker) {
                g.push((TAG_FS, c.0));
            }
        }
        if self.env.proc_broker.0 != 0 {
            if let Ok(c) = tinyos_app::broker::connect(self.env.proc_broker) {
                g.push((TAG_PROC, c.0));
            }
        }
        for (tag, br) in [
            (TAG_FS_BROKER, self.env.fs_broker.0),
            (TAG_PROC_BROKER, self.env.proc_broker.0),
        ] {
            if br != 0 {
                if let Ok(h) = syscall2(SYS_HANDLE_DUP, br as u64, RIGHTS_ALL as u64).ok() {
                    g.push((tag, h as u32));
                }
            }
        }
        g
    }
```

- [ ] **Step 3: Build the apps workspace**

Run: `cd apps && cargo build --release`
Expected: `Finished`.

- [ ] **Step 4: Commit**
```bash
git add apps/shell/src/main.rs
git commit -m "sh: mint a fresh FS/PROC connection per child via the broker

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 9: Integration — build the disk, run the smoke harness, add a regression step

**Files:**
- Modify: `tools/smoke/smoke.py` (add one step)

**Interfaces:**
- Consumes: the whole system built by Tasks 1-8.

- [ ] **Step 1: Full build + sync the apps into the disk image**

Run: `make sync-apps`
Expected: builds kernel + apps, `mkfs-tinyfs put` for each app; no errors.

- [ ] **Step 2: Run the existing smoke harness (unchanged) — the primary gate**

Run: `make smoke`
Expected: `smoke: PASS`. This already exercises the rewired paths: `sh` uses its own FS/PROC connection (`write`/`cat`/`ls`/`ps`), spawns children through the broker (`run hello alpha beta`, `run hello &`, `run top`), and the reboot-durability check. If the broker wiring is wrong, spawns hang or FS ops fail and the harness times out.

- [ ] **Step 3: Add a targeted regression step (two live processes, separate connections)**

`tools/smoke/smoke.py` — after the background-job block (after the `serial.wait_for("hello done", ...)` / `print("smoke: background job reaped")` lines), insert:
```python
        # Broker regression: a background child is alive (holding its OWN
        # broker-minted FS/PROC connection) while the foreground shell does FS
        # work on ITS connection. Pre-broker this shared one channel; now they
        # are isolated. Both must produce correct output.
        step("bg child + fg fs", "run hello &", "] hello &")
        step("fg write while child alive", "write /broker.txt isolated")
        step("fg read while child alive", "cat /broker.txt", "[out] isolated")
```

- [ ] **Step 4: Re-run the smoke harness with the new step**

Run: `make smoke`
Expected: `smoke: PASS`, including the three new `smoke: >` lines and `[out] isolated`.

- [ ] **Step 5: Final both-arch compile check**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`.

- [ ] **Step 6: Commit**
```bash
git add tools/smoke/smoke.py
git commit -m "smoke: regression step for isolated per-child FS connections

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-review notes

- **Spec coverage:** broker protocol (Task 1), kernel FS/PROC servers + boot pump (Tasks 4-6), rewired kernel-terminal spawner (Task 7), SDK `broker::connect` + `Env` (Task 2), rewired `sh` (Task 8), reaping via `is_open` (Task 3), smoke gate (Task 9). Non-goals (CONSOLE/WINDOW brokers, isolation policy, userspace terminal) are untouched by design.
- **Same-thread mint (spec crux):** honored — the kernel terminal calls `svc::mint_fs/mint_proc` (direct), never the broker channel; only `sh` (cross-thread) uses `broker::connect`.
- **Boot order:** `svc::init()` runs in `main()` before `sched::start`, so the servers exist before `ui_thread_main` constructs the shell and spawns `sh`.
- **Reaping:** `FsServer::pump`/`ProcServer::pump` drop connections whose `is_open()` is false (client handle dropped on process teardown), bounding pool growth.
- **Types consistent:** `mint()`/`*_broker_handle()` return `Handle`; grants are `Vec<(u32, Handle)>` for `spawn_with_grants` (kernel) and `&[(u32, u32)]` for `process::spawn` (userspace) — matching each call site.
