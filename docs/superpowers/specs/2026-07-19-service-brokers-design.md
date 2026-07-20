# Service brokers + per-client connections (SP0)

Date: 2026-07-19
Status: design, approved direction — pending written-spec review
Prerequisite for: userspace terminal (SP1) → App-trait deletion (SP2)

## Context & motivation

Today the kernel terminal (`kernel/src/term/mod.rs`) is the FS/PROC server for
the apps it hosts. When it spawns `sh`, `loader::spawn(GrantSet::all())` creates
console/shell/fs/proc channel pairs, grants the app ends to `sh`, and binds a
`FsService` / `ProcService` to the kernel ends (`term/mod.rs:697`). When `sh`
spawns a child (`vi`, `top`), `child_grants()` **dups its own fs/proc channel**
and grants the dup (`apps/shell/src/main.rs:78`). So the whole process tree
shares *one* FS channel and *one* PROC channel back to a single service instance.

That has two defects:

1. **Request/reply collision (correctness).** A single request/reply channel
   shared by multiple clients is only safe because the foreground-blocking model
   serializes them — `sh` is parked in `child.wait()` while `vi` runs. Background
   jobs (`run x &`) already break that assumption: a background job and the
   foreground app can both issue FS requests on the same channel and receive each
   other's replies. The bug is latent today; forwarding the model to a userspace
   terminal would add another sharer.
2. **No per-app isolation (security).** A child inherits the *full* FS/PROC
   capability by dup, not a narrowed one. There is no seam to scope a child to a
   subtree or to hand it a read-only PROC view.

We are about to move the terminal to userspace (SP1) and delete the in-kernel App
model (SP2). This is the moment to fix the capability plumbing rather than carry
the debt one level higher.

## The model: service brokers + per-client connections

Model each kernel service as a **broker channel** that mints fresh, isolated
connections instead of sharing one:

- The kernel runs a standing **FS server** and **PROC server**. Each owns a
  *broker* channel. A client sends `OP_CONNECT`; the server mints a new service
  instance bound to a fresh channel pair, keeps the server end in its connection
  pool, and replies with the **client end as a moved handle**.
- **Spawning a child** (kernel terminal, `sh`, later the userspace terminal):
  mint a *fresh* connection per child via `OP_CONNECT` and grant it as
  `TAG_FS` / `TAG_PROC`. Also forward the *broker* handle (dup) as
  `TAG_FS_BROKER` / `TAG_PROC_BROKER` so the child can mint for *its* children.
- **Leaf apps are unchanged.** They still receive `TAG_FS` / `TAG_PROC` as a
  ready-to-use connection. Only *spawners* change.

This needs **no new syscalls or kernel object types** — moved handles already
travel inside `channel::Message { bytes, handles: Vec<Handle> }`
(`kernel/src/obj/channel.rs:20`), a message can carry an `Object::Channel`
(`kernel/src/obj/mod.rs:21`), and the receiver's `sys_channel_recv` installs the
moved handle into the caller's table and returns a fresh id
(`kernel/src/obj/syscall.rs:256`). It is the exact reverse of how the compositor
receives a MemObj from `window::open` (`extern_app.rs:111`).

## Goals

- Every app gets its own 1:1 FS and PROC connection — no shared request/reply
  channel, no cross-client collision, correct under background jobs.
- Establish the broker seam so (a) connections can later be scoped
  (dir-rooted FS, read-only PROC) as pure policy, and (b) the kernel FS/PROC
  servers can later be re-hosted as userspace processes serving the *same*
  broker protocol (the deferred `fsd` item) with nothing upstream changing.
- Behavior-preserving: same authority granted as today (full-root FS, `can_kill`
  PROC), same boot, both arches, kernel terminal still the default.

## Non-goals (explicitly out of scope for SP0)

- Actual isolation *policy* — SP0 mints full-root FS connections and `can_kill`
  PROC connections, exactly as today. Scoping is a later layer.
- Applying the broker model to CONSOLE / WINDOW / SHELL — those stay direct
  channels for now. (CONSOLE moves in SP1 when the terminal goes to userspace.)
- The userspace terminal (SP1) and App-trait deletion (SP2).
- A general service directory / powerbox — separate FS and PROC brokers, YAGNI.

## Detailed design

### ABI additions

New `crates/abi/src/broker.rs` (broker protocol v0):

```
OP_CONNECT   = 1   // client -> server: request a new connection
R_CONNECTED  = 2   // server -> client: {status:u32}, + moved handle (client end) on success
BROKER_OK    = 0
BROKER_NOMEM = 1
```

`crates/abi/src/bootstrap.rs` — two new tags:

```
TAG_FS_BROKER   = 5
TAG_PROC_BROKER = 6
```

(Existing `TAG_CONSOLE=1, TAG_SHELL=2, TAG_FS=3, TAG_PROC=4` unchanged.)

### Kernel: FS server and PROC server

Two new standing servers, each = a broker channel end + a pool of live
connections. Pumped by the `ui_thread` each iteration (like the shell is).

`kernel/src/fs/server.rs` (new):

```rust
pub struct FsServer {
    broker: Arc<ChannelEnd>,          // kernel end of the broker channel
    conns:  Vec<FsService>,           // one per live client connection
}

impl FsServer {
    pub fn new(broker: Arc<ChannelEnd>) -> Self { ... }

    /// Mint one fresh connection: create a pair, bind a service to the server
    /// end, pool it, return the CLIENT end as a handle (RIGHT_TRANSFER retained
    /// so it survives broker->spawner->child). The single connection-creation
    /// path, shared by both callers below.
    pub fn mint(&mut self) -> Handle {
        let (client, server) = channel::create();
        self.conns.push(FsService::new(server, "/".into(), "/".into()));
        Handle::new(Object::Channel(client), RIGHTS_ALL)
    }

    /// Drain broker connect-requests, then pump every live connection.
    /// Reap connections whose client end has been dropped.
    pub fn pump(&mut self) {
        while let Ok(msg) = self.broker.recv() {
            if op(&msg) == OP_CONNECT {
                let h = self.mint();
                let reply = /* R_CONNECTED + BROKER_OK */;
                let _ = self.broker.send(Message { bytes: reply, handles: vec![h] });
            }
        }
        self.conns.retain_mut(|c| { c.pump(); c.is_open() });  // reap dead peers
    }
}
```

`kernel/src/obj/procserver.rs` (new): identical shape, holding `Vec<ProcService>`,
minting `ProcService::new(server, /*can_kill=*/true)` per connection (SP0 keeps
`can_kill=true` for all — same as today; per-connection privilege is a later
policy).

**Two entry points, one mint path — this is the crux of the design.** In-kernel
callers on the `ui_thread` (the kernel terminal) call `mint()` **directly** — they
must not round-trip through the broker channel, because that same thread pumps the
server and would deadlock waiting for a reply. Cross-thread userspace clients
(`sh`, later the userspace terminal) use the broker *channel* (`OP_CONNECT`),
which `pump()` services by calling the same `mint()`. Both yield an isolated
connection; only the delivery differs.

Connection reaping needs a cheap "is the client end still alive?" check. The
channel already holds a `Weak` peer link (`channel.rs`); add `ChannelEnd::peer_alive()`
(upgrade the weak ref) and expose `FsService::is_open()` / `ProcService::is_open()`
delegating to it. This bounds pool growth as apps come and go.

### Kernel boot wiring

In `main.rs` boot (near `fs::init`): create the broker channel pairs, construct
`FsServer` / `ProcServer` on the kernel ends, stash them where the `ui_thread`
can pump them (alongside `UI_STATE`), and keep the *client* broker ends to hand
to the first spawner. `ui_thread_main`'s loop gains `fs_server.pump()` /
`proc_server.pump()` next to `shell.pump_externals()`.

### Rewire the kernel terminal

`term::spawn_app` stops binding a per-child `FsService`/`ProcService`. Instead,
for each child it:

1. Calls `FsServer::mint()` / `ProcServer::mint()` **directly** (same-thread as
   the server pump — see "two entry points" above; a channel round-trip would
   deadlock) to get client-end handles.
2. Grants those client ends as `TAG_FS` / `TAG_PROC`, plus the broker client
   channel ends (held from boot, duped with `RIGHT_TRANSFER` retained) as
   `TAG_FS_BROKER` / `TAG_PROC_BROKER` so the child can mint for its own children.
3. Keeps only console/shell wiring as before.

`RunningApp` loses its `fs_srv` / `proc_srv` fields and the terminal no longer
pumps per-child services (the servers do). This is a net simplification of
`term/mod.rs`.

Because `loader::spawn(GrantSet)` hard-codes creating fs/proc pairs, the terminal
switches to `loader::spawn_with_grants(...)` with an explicitly built grant list
(console + shell created as today; fs/proc/brokers from the broker mints). The
`GrantSet` convenience path is updated or bypassed accordingly.

### SDK: broker client

`apps/sdk/src/broker.rs` (new):

```rust
/// Ask a broker for a fresh service connection.
pub fn connect(broker: Channel) -> Result<Channel, u32> {
    broker.send(&OP_CONNECT.to_le_bytes(), &[])?;
    let msg = broker.recv()?;                 // blocks; brokers reply promptly
    // status in msg.bytes[4..8]; connection rides as msg.handles[0]
    match status(&msg) {
        BROKER_OK => Ok(Channel(msg.handles[0])),
        st => Err(st),
    }
}
```

`apps/sdk/src/entry.rs` — `Env` gains `fs_broker: Channel` and
`proc_broker: Channel` (0 when absent), populated from `TAG_FS_BROKER` /
`TAG_PROC_BROKER` in the bootstrap record. Leaf apps ignore them.

### Rewire sh

`apps/shell/src/main.rs` `child_grants()` changes from dup-sharing to
per-child minting:

```rust
fn child_grants(&self) -> Vec<(u32, u32)> {
    let mut g = Vec::new();
    // console + shell: dup as today (CONSOLE moves to a broker in SP1).
    for (tag, ch) in [(TAG_CONSOLE, self.env.console.0), (TAG_SHELL, self.env.shell.0)] {
        if ch != 0 { if let Ok(h) = dup(ch) { g.push((tag, h)); } }
    }
    // fs/proc: mint a FRESH connection per child from the broker.
    if let Ok(c) = broker::connect(self.env.fs_broker) { g.push((TAG_FS, c.0)); }
    if let Ok(c) = broker::connect(self.env.proc_broker) { g.push((TAG_PROC, c.0)); }
    // forward the brokers so the child can mint for ITS children.
    for (tag, br) in [(TAG_FS_BROKER, self.env.fs_broker.0), (TAG_PROC_BROKER, self.env.proc_broker.0)] {
        if br != 0 { if let Ok(h) = dup(br) { g.push((tag, h)); } }
    }
    g
}
```

The minted connection handle is consumed by spawn (moved); the broker handle is
duped so `sh` keeps its own for the next child.

## Security properties

**Improved now:**
- Per-app 1:1 connections: a background job can no longer race the foreground app
  on a shared FS/PROC channel. Correctness under concurrency.
- Authority is now an explicit, named capability (the broker) that is forwarded
  deliberately, rather than an ambiently-dup'd service channel.

**Enabled for later (not done in SP0):**
- Scoped FS: a broker (or an `OP_CONNECT` argument) can mint an `FsService` with
  a non-`/` jail — `FsService` already supports it. Dir-caps / jails become a
  policy layer on top of this seam.
- Restricted PROC: mint `can_kill=false` (read-only) connections for untrusted
  apps and a privileged one for the shell — a per-connection property once we
  want it.

**Unchanged in SP0 (deliberately broad):** every connection is full-root FS and
`can_kill` PROC, exactly as today. SP0 fixes structure, not policy.

## Testing

- **Host tests:** `FsServer`/`ProcServer` connect + reap logic where extractable;
  the broker request/reply encoding. The services themselves are already tested.
- **`make smoke` (unchanged assertions, still the kernel terminal):** the
  existing script already exercises the exact defect — `run hello &` (background
  job) followed by fs/proc commands, `ps`, `write`/`cat`, and the reboot
  durability check. After the rewire, `sh` and `vi`/`hello` each get their own
  FS/PROC connection; smoke must stay green. Add one targeted step: a background
  job doing FS work concurrent with a foreground FS command, to assert no
  cross-talk (the bug this fixes).
- **Both arches:** `cargo check` aarch64 + x86_64 as always.

## Rollout / migration

Behavior-preserving and always-bootable:
1. ABI (broker protocol + tags) — additive.
2. Kernel FS/PROC servers + boot wiring + `ui_thread` pump — servers exist,
   nothing uses them yet.
3. SDK `broker::connect` + `Env` broker fields — additive.
4. Rewire `term::spawn_app` to mint+grant brokers; rewire `sh` `child_grants`.
   This is the switch-over commit; smoke gates it.
5. Remove now-dead per-child service wiring from `term`.

The kernel terminal remains the default throughout. No app binary except `sh`
changes behavior; leaf apps recompile against the new `Env` unchanged.

## Risks

- **Rights on the minted handle.** The broker reply must mint the connection
  handle with `RIGHT_TRANSFER` retained (so it survives broker→spawner→child).
  `RIGHTS_ALL` (0x3F) includes it; `dup` only narrows. Spelled out so it isn't
  lost.
- **Connection-pool growth / reaping.** Without reaping, `conns` grows per
  spawned app. The `peer_alive()` check bounds it; verify a killed/exited app's
  connection is actually reaped (its client-end handle is dropped when the
  process handle table is cleared).
- **Same-thread mint must be direct (designed, not incidental).** The `ui_thread`
  pumps the servers, so any in-kernel caller on that thread (the kernel terminal)
  must use `FsServer::mint()` directly — a broker *channel* round-trip would block
  the pump waiting for its own reply. Userspace `sh` mints are normal cross-thread
  round-trips and are fine. The design handles this with the two-entry-point mint
  (above); the plan must preserve the split and never route an in-kernel mint
  through the channel.
- **Boot-order dependency for the pump.** The FS/PROC servers must be created and
  registered for pumping before the kernel terminal spawns `sh` (which mints).
  Sequence boot so `fs_server`/`proc_server` exist and are reachable by
  `ui_thread_main` before the shell constructs.

## Out of scope → future

- SP1: userspace terminal (forwards brokers like `sh` does; CONSOLE becomes a
  broker/owned channel there).
- SP2: delete App trait, flip default, migrate serial mirror, compositor respawn.
- Later: dir-scoped FS brokers, read-only PROC, userspace `fsd` serving the same
  broker protocol.
