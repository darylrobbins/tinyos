# Kernel-attested app identity + windowed children (exec-by-path) — SP1c

Date: 2026-07-20
Status: design, approved direction — pending written-spec review
Supersedes: the earlier "window broker with parent-asserted identity" sketch
Builds on: SP1b (userspace terminal surfaces, merged)
Precedes: SP1d (live regions), SP3 (userspace compositor + identity-as-capability)

## Context & motivation

`uterm`'s `sh` can host console-surface children (vi, top) but not *windowed*
children (edit, pixels): window channels are 1:1 and the terminal grants `sh`
none. The obvious fix — a window broker where `sh` mints a window channel and
names it — hits a real problem: the window chrome shows a **trusted identity**
(the kernel-registered process name, semibold) next to the app-claimed title,
specifically "so no app can pose as another." tinyOS has **no way to attribute a
channel message to a sender** (messages/ends carry no PID; `Object` has no owner
back-pointer; a process's name is set by its spawner). So a broker-minted window
would carry a name a *userspace* process (`sh`) chose — downgrading identity from
kernel-attested to parent-asserted, and letting a dishonest `sh` label a window
`"Terminal"`.

The root cause is that tinyOS has no kernel-attested app identity at all. The
`sh` spawn path (`SYS_PROCESS_SPAWN`) hands the kernel *raw ELF bytes* plus a
name derived from `argv[0]` — all caller-controlled. (The desktop launcher
`SvcJob::spawn` is the exception: it reads `/apps/<name>` in the kernel, so it
*knows* what it loaded.)

SP1c fixes the root cause: the kernel loads apps **by path** (like `SvcJob`
already does) and attests identity from what it resolved. This makes windowed
children fall out cleanly — the window is minted **at exec, under the
kernel-attested identity** — with no broker, no name field, no provenance hack.

## Design principles

- **Identity is kernel-attested; authority stays parent-delegated.** The kernel
  decides *who* an app is (from the path it loaded). The parent still decides
  *what* it can do (the grants it passes). The kernel does **not** become a
  policy engine — it does not decide what channels an app gets from a manifest.
- **A window is authority the parent delegates + a need the child declares.**
  The kernel mints a window (under the attested identity) only when the parent
  requests one **and** the child's manifest declares the `window` capability —
  mirroring `SvcJob`'s manifest ∩ policy, so console apps get no spurious window.

## Detailed design

### New syscall: `SYS_PROCESS_EXEC` (= 14)

```
SYS_PROCESS_EXEC(argv_ptr, argv_len, grants_ptr, grant_count, out_ptr, flags)
```
Same shape as `SYS_PROCESS_SPAWN` (13) except: **no ELF MemObj** — `argv[0]` is
the app *path* (`/apps/edit`), which the kernel resolves and reads itself; and a
`flags` word (bit 0 = `EXEC_REQUEST_WINDOW`).

Kernel handler (`kernel/src/obj/syscall.rs`, alongside `sys_process_spawn`):
1. Parse `argv` (reuse the spawn record parser). `argv[0]` = path.
2. `let elf = crate::fs::read("/", path)?;` — ambient kernel FS (canonical
   `/apps`, exactly as `SvcJob::spawn` does). Not the caller's jail — noted as a
   future tightening.
3. **Attested identity** = the path's basename (`/apps/edit` → `edit`). This is
   the process name passed to `loader::spawn_with_grants`, overriding the
   `argv[0]`-derived name the spawn path uses.
4. Move the caller's grants (`(tag, handle)` pairs, `RIGHT_TRANSFER` required),
   identical to `sys_process_spawn`.
5. **Window mint** — if `flags & EXEC_REQUEST_WINDOW` **and**
   `loader::manifest(&elf).window`: create a shell channel pair,
   `extern_app::register(kernel_end, attested_name)`, and add `(TAG_SHELL,
   app_end)` to the child's grants. The window is registered under the
   *attested* name; `sh` never names it.
6. `loader::spawn_with_grants(attested_name, &elf, argv, grants)`; return
   `(proc_h, main_h)` via `out_ptr`, exactly as spawn.

`register()` is a global-queue push (`extern_app.rs`), safe to call from the
caller's thread; the compositor drains it on the ui_thread (as it already does
for every spawner). No round-trip, no deadlock.

`SYS_PROCESS_SPAWN` (raw bytes) stays for internal/test callers, but its
identity remains `argv[0]`-derived (unattested) — anything that wants attested
identity uses exec.

### SDK

`apps/sdk/src/process.rs` gains:
```rust
/// Exec `/apps/…`-style path: the KERNEL loads it (attesting identity from the
/// path) and delegates the given grants. `want_window` asks the kernel to mint
/// a window under the attested identity (honored iff the app declares `window`).
pub fn exec(path: &str, args: &[&str], grants: &[(u32, u32)], want_window: bool)
    -> Result<Child, u32>;
```
It builds the argv record (`argv[0] = path`, then `args`), marshals grants, and
invokes `syscall6(SYS_PROCESS_EXEC, argv_ptr, argv_len, gr_ptr, grant_count,
out_ptr, if want_window { EXEC_REQUEST_WINDOW } else { 0 })`. No ELF staging —
the kernel reads the file, so the SDK helper is simpler than `spawn`.

`abi::syscall`: `SYS_PROCESS_EXEC = 14`, `EXEC_REQUEST_WINDOW = 1`.

### sh

`run` switches from "read the ELF + `process::spawn`" to `process::exec`:
```rust
fn run(&mut self, name: &str, args: &[&str], background: bool) {
    let grants = self.child_grants();       // fs/proc + brokers, unchanged
    match process::exec(&format!("/apps/{name}"), args, &grants, /*want_window=*/ true) {
        Ok(child) => { /* foreground/background as today */ }
        Err(FS_NOT_FOUND) => err(&format!("run: {name}: not found")),
        Err(st) => err(&format!("run: spawn failed (status {st})")),
    }
}
```
`sh` no longer reads `/apps/<name>` for spawning (the kernel does), and no longer
needs a window channel or window broker. `want_window = true` for every `run`;
the kernel mints one only for apps that declare `window` (edit, pixels), so
console apps (vi, top, hello) get none. `child_grants()` is unchanged (console
dup, FS/PROC minted from brokers, brokers forwarded).

### launch_uterm & the terminal — unchanged

`launch_uterm` is trusted in-kernel code; it keeps registering the terminal's
own window as `"Terminal"` directly. The terminal keeps spawning `sh` as it does
(sh opens no window, so its identity is immaterial). SP1c changes only the
*userspace `run` path*, which is where unattested identity actually mattered.

## Security properties

- **Identity is attested by the filesystem.** A window's trusted name is the
  basename of the `/apps/…` binary the kernel resolved. To make a window claim
  `"Terminal"`, an attacker must place a binary at that identity in `/apps` —
  which requires FS-write, itself a capability. The bar rises from "free string,
  zero cost" (parent-asserted) to "control a binary at that path."
- **No new ambient authority.** The parent still delegates all handle grants;
  the kernel only *names* the process and mints the one window the parent asked
  for and the child declared. The window-mint is gated by
  parent-request ∧ manifest — the same shape as `SvcJob`.
- **Residual, documented:** ambient `/apps` resolution ignores the caller's FS
  jail (a jailed process could exec any `/apps` binary). Acceptable while `/apps`
  is the trusted app store; the jail-clean version (resolve through the caller's
  FS capability) is future work. And identity is not yet a transferable
  *capability* — that lands with the userspace compositor (below).

## Forward compatibility (the endgame this sets up)

When the compositor moves to userspace (SP3, per
`docs/architecture/display-and-windowing.md`), identity must travel to a
userspace window server. The natural extension: exec mints an **identity
capability** (a kernel-minted token "bearer is `edit`") granted to the child,
which it presents to the display server when opening a window; the server trusts
it because it is kernel-minted — the same attestation, now transferable. SP1c
lays the load-time attestation; SP3 turns it into a capability. Nothing here
needs redoing for that.

## Testing

- **`make smoke`** (the attestation is serial-visible via `ps`): launch `uterm`,
  `run pixels &`, then `ps` — assert a process named **`pixels`** appears
  (attested from `/apps/pixels`, not an `argv[0]` claim) and the job stays alive.
  This verifies exec-by-path attestation on serial even though the window itself
  is framebuffer-only. Assert no kernel panic.
- **Manual QEMU:** `run edit` / `run pixels` from `uterm` open their own
  top-level windows (titled `edit` / `pixels`), alongside `uterm`; `vi`/`top`
  still render *inside* `uterm` (SP1b); the chrome identity matches `ps`.
- **Both arches:** `cargo check` aarch64 + x86_64 (kernel syscall change).
- Host tests: the basename/attestation is trivial kernel logic; if a pure helper
  is extracted (path → basename), unit-test it.

## Out of scope → future

- SP1d: live regions (`OP_LIVE_*`, progress-style bottom panel in `uterm`).
- SP3: userspace compositor; identity-as-capability (exec mints a transferable
  identity token; the display server consumes it).
- Jail-clean exec (resolve the path through the caller's FS capability).
- Multiple windows per app (exec mints one; a runtime window mechanism if ever
  needed).
- Also pending (SP0 review): per-request broker reply channel before scoped FS.
