# Kernel-Attested App Identity (exec-by-path) — SP1c Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `SYS_PROCESS_EXEC`: the kernel loads an app by path, attests its identity from the resolved `/apps` basename, and mints the child's window under that attested identity — so `sh`'s `run edit`/`run pixels` open their own windows with a trusted, un-spoofable name.

**Architecture:** A new syscall parallel to `SYS_PROCESS_SPAWN`, but the caller passes a path (as `argv[0]`) instead of a staged ELF MemObj. The kernel reads the file itself (ambient `/apps`, like `SvcJob`), names the process from the basename, moves the caller's grants unchanged (authority stays parent-delegated), and — if the parent set `EXEC_REQUEST_WINDOW` and the app's manifest declares `window` — mints a window channel registered under the attested name and grants it as `TAG_SHELL`. `sh`'s `run` switches to `process::exec`.

**Tech Stack:** Rust `no_std` (kernel: `aarch64-unknown-uefi` + `x86_64-unknown-uefi`; apps: `aarch64-unknown-none`), `tinyos-abi`, `make` + QEMU + `make smoke`.

## Global Constraints

- Both kernel targets compile: `cargo check -p kernel --target aarch64-unknown-uefi` AND `--target x86_64-unknown-uefi`.
- **Identity kernel-attested; authority parent-delegated.** Exec derives the process name from the resolved path basename (NOT `argv[0]`-as-claimed, NOT a manifest name). Grants are moved from the caller exactly as `sys_process_spawn` does — no new ambient authority.
- **Window-mint gate:** mint a window ONLY when `flags & EXEC_REQUEST_WINDOW` AND `loader::manifest(&elf).window`. (`Manifest::legacy()` — apps with no `declare_caps!` — has `window: true`, so `pixels` qualifies; `edit` declares `window` explicitly.)
- **Exec argv convention:** the record's `argv[0]` is the app PATH (kernel-only: used to read the file + derive identity). The child's bootstrap argv is `argv[1..]` — so apps see their arguments unchanged (tinyOS apps do not use `argv[0]` as a program name).
- `SYS_PROCESS_SPAWN` (raw bytes) stays for internal/test callers; only `sh`'s `run` migrates to exec.
- Path resolution is ambient kernel FS (`crate::fs::read("/", path)`), canonical `/apps`, exactly as `SvcJob::spawn`. (Jail-clean resolution is future.)
- Commit trailer on its own line after a blank line: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Spec: `docs/superpowers/specs/2026-07-20-kernel-attested-identity-design.md`.

---

## File structure

- `crates/abi/src/syscall.rs` (modify) — `SYS_PROCESS_EXEC = 14`, `EXEC_REQUEST_WINDOW = 1`.
- `kernel/src/obj/syscall.rs` (modify) — extract grant-move helper; add `sys_process_exec`; dispatch arm.
- `apps/sdk/src/process.rs` (modify) — `exec(path, args, grants, want_window)`.
- `apps/shell/src/main.rs` (modify) — `run` uses `process::exec`.
- `tools/smoke/smoke.py` (modify) — attestation check via `ps`.

---

### Task 1: ABI — `SYS_PROCESS_EXEC` + flag

**Files:**
- Modify: `crates/abi/src/syscall.rs` (after `SYS_PROCESS_SPAWN: u64 = 13;` — note 14 is free, 15 is `SYS_MEMOBJ_UNMAP`)

**Interfaces:**
- Produces: `abi::syscall::{SYS_PROCESS_EXEC: u64 = 14, EXEC_REQUEST_WINDOW: u64 = 1}`.

- [ ] **Step 1: Add the constants**

`crates/abi/src/syscall.rs` — add after line 27 (`pub const SYS_PROCESS_SPAWN: u64 = 13;`):
```rust
/// Like SYS_PROCESS_SPAWN, but the kernel loads the app BY PATH (argv[0]),
/// attesting identity from the resolved /apps basename. flags bit 0 =
/// EXEC_REQUEST_WINDOW: mint a window under the attested identity (honored iff
/// the app's manifest declares `window`).
pub const SYS_PROCESS_EXEC: u64 = 14;

/// flags bit for SYS_PROCESS_EXEC: request a window for the child.
pub const EXEC_REQUEST_WINDOW: u64 = 1;
```

- [ ] **Step 2: Build the abi crate**

Run: `cargo build -p tinyos-abi`
Expected: `Finished`.

- [ ] **Step 3: Commit**
```bash
git add crates/abi/src/syscall.rs
git commit -m "abi: SYS_PROCESS_EXEC + EXEC_REQUEST_WINDOW

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Kernel — `sys_process_exec`

**Files:**
- Modify: `kernel/src/obj/syscall.rs` (extract a grant-move helper from `sys_process_spawn`; add `sys_process_exec`; add the dispatch arm at line ~53)

**Interfaces:**
- Consumes: `crate::fs::read`; `crate::obj::loader::{spawn_with_grants, manifest}`; `crate::obj::channel::create`; `crate::obj::handle::{Handle, RIGHTS_ALL}`; `crate::obj::Object`; `crate::ui::shell::extern_app::register`; `abi::bootstrap::TAG_SHELL`; `abi::syscall::{SYS_PROCESS_EXEC, EXEC_REQUEST_WINDOW}`.

- [ ] **Step 1: Extract the grant-move logic into a helper**

In `sys_process_spawn`, the block that moves `(tag, handle)` grants out of the caller's table (from `let graw = copy_in(grants_ptr, grant_count * 8)?;` through the rollback) is duplicated by exec. Extract it. Add near `sys_process_spawn`:
```rust
/// Move `grant_count` (tag, handle) pairs out of process `p`'s handle table,
/// requiring RIGHT_TRANSFER, with full rollback on any failure. The caller
/// then hands the returned Vec to loader::spawn_with_grants.
fn take_grants(p: &alloc::sync::Arc<Process>, grants_ptr: u64, grant_count: u64)
    -> Result<Vec<(u32, Handle)>, u32>
{
    let graw = copy_in(grants_ptr, grant_count * 8)?;
    let mut grants: Vec<(u32, Handle)> = Vec::new();
    let mut taken: Vec<(u32, u32)> = Vec::new();
    let mut t = p.handles.lock();
    for i in 0..grant_count as usize {
        let tag = u32::from_le_bytes(graw[i * 8..i * 8 + 4].try_into().unwrap());
        let hv = u32::from_le_bytes(graw[i * 8 + 4..i * 8 + 8].try_into().unwrap());
        match t.take(hv) {
            Ok(h) if h.rights & RIGHT_TRANSFER != 0 => {
                taken.push((hv, tag));
                grants.push((tag, h));
            }
            Ok(h) => {
                t.insert_back(hv, h);
                for ((hv2, _), (_, h2)) in taken.iter().zip(grants.drain(..)) {
                    t.insert_back(*hv2, h2);
                }
                return Err(ST_ACCESS_DENIED);
            }
            Err(e) => {
                for ((hv2, _), (_, h2)) in taken.iter().zip(grants.drain(..)) {
                    t.insert_back(*hv2, h2);
                }
                return Err(e);
            }
        }
    }
    Ok(grants)
}
```
Then replace that inline block in `sys_process_spawn` with `let grants = take_grants(&p, grants_ptr, grant_count)?;` (keep everything else in spawn identical). Verify spawn still compiles + behaves (the smoke run in Task 5 covers it).

- [ ] **Step 2: Add `sys_process_exec`**

`kernel/src/obj/syscall.rs`:
```rust
fn sys_process_exec(
    argv_ptr: u64,
    argv_len: u64,
    grants_ptr: u64,
    grant_count: u64,
    out_ptr: u64,
    flags: u64,
) -> SysResult {
    const MAX_ARGV: u64 = 4096;
    const MAX_ARGS: u32 = 16;
    const MAX_GRANTS: u64 = 8;
    if argv_len > MAX_ARGV || grant_count > MAX_GRANTS {
        return Err(ST_INVALID_ARGS);
    }
    let p = cur_proc()?;

    // argv record: u32 argc, then per arg u32 len + utf8. argv[0] = path.
    let record = copy_in(argv_ptr, argv_len)?;
    let u32at = |o: usize| -> Result<u32, u32> {
        record.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or(ST_INVALID_ARGS)
    };
    let argc = u32at(0)?;
    if argc == 0 || argc > MAX_ARGS {
        return Err(ST_INVALID_ARGS);
    }
    let mut all: Vec<String> = Vec::with_capacity(argc as usize);
    let mut off = 4usize;
    for _ in 0..argc {
        let len = u32at(off)? as usize;
        off += 4;
        let bytes = record.get(off..off + len).ok_or(ST_INVALID_ARGS)?;
        all.push(core::str::from_utf8(bytes).map_err(|_| ST_INVALID_ARGS)?.into());
        off += len;
    }
    let path = all[0].clone();
    let child_argv: Vec<String> = all[1..].to_vec();

    // Kernel-attested identity: the resolved path's basename.
    let name = path.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(&path).to_string();

    // The KERNEL reads the app (ambient /apps, like SvcJob) — this is the
    // attestation: identity comes from what the kernel resolved, not a claim.
    let elf = match crate::fs::read("/", &path) {
        Ok(e) => e,
        Err(_) => return Err(ST_INVALID_ARGS), // missing/unreadable app
    };

    // Authority: move the caller's grants, unchanged.
    let mut grants = take_grants(&p, grants_ptr, grant_count)?;

    // Window: parent-requested AND app-declared → mint under the attested name.
    if flags & EXEC_REQUEST_WINDOW != 0 && crate::obj::loader::manifest(&elf).window {
        let (app_end, kern_end) = crate::obj::channel::create();
        crate::ui::shell::extern_app::register(kern_end, name.clone());
        grants.push((
            abi::bootstrap::TAG_SHELL,
            Handle::new(Object::Channel(app_end), RIGHTS_ALL),
        ));
    }

    let (child, tid, main_peer) =
        match crate::obj::loader::spawn_with_grants(name, &elf, &child_argv, grants) {
            Ok(r) => r,
            Err(_) => return Err(ST_INVALID_ARGS),
        };
    let (ph, mh) = {
        let mut t = p.handles.lock();
        let ph = t.insert(Handle::new(Object::Process(child), RIGHT_WAIT | RIGHT_DUP | RIGHT_TRANSFER))?;
        let mh = t.insert(Handle::new(Object::Channel(main_peer), RIGHTS_ALL))?;
        (ph, mh)
    };
    copy_out_u32s(out_ptr, &[ph, mh])?;
    Ok(tid as u64)
}
```
(Ensure `SYS_PROCESS_EXEC` and `EXEC_REQUEST_WINDOW` are in scope via the file's `use abi::syscall::*`. `ST_INVALID_ARGS`, `ST_ACCESS_DENIED`, `RIGHT_TRANSFER`, `RIGHT_WAIT/DUP`, `RIGHTS_ALL`, `copy_in`, `copy_out_u32s`, `cur_proc`, `Handle`, `Object` are all already used by `sys_process_spawn` in this file. There is NO `ST_NOT_FOUND` status — do not reference one.)

- [ ] **Step 3: Add the dispatch arm**

`kernel/src/obj/syscall.rs` — after the `SYS_PROCESS_SPAWN => { … }` arm (line ~55):
```rust
        SYS_PROCESS_EXEC => {
            sys_process_exec(args[0], args[1], args[2], args[3], args[4], args[5])
        }
```

- [ ] **Step 4: Check both kernel targets compile**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`.

- [ ] **Step 5: Commit**
```bash
git add kernel/src/obj/syscall.rs
git commit -m "kernel: sys_process_exec — load by path, attest identity, mint window

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: SDK — `process::exec`

**Files:**
- Modify: `apps/sdk/src/process.rs`

**Interfaces:**
- Consumes: `abi::syscall::{SYS_PROCESS_EXEC, EXEC_REQUEST_WINDOW}` (Task 1); `crate::syscall::syscall6`.
- Produces: `tinyos_app::process::exec(path: &str, args: &[&str], grants: &[(u32, u32)], want_window: bool) -> Result<Child, u32>`.

- [ ] **Step 1: Add `exec`**

`apps/sdk/src/process.rs` — add alongside `spawn`:
```rust
/// Exec a `/apps/…`-style path. The KERNEL loads it (attesting identity from
/// the path) and delegates `grants` unchanged. `want_window` asks the kernel to
/// mint a window under the attested identity (honored iff the app declares
/// `window`). The child sees `args` as its argv (the path is kernel-only).
pub fn exec(path: &str, args: &[&str], grants: &[(u32, u32)], want_window: bool) -> Result<Child, u32> {
    // argv record: argv[0] = path (kernel reads it), then the child's args.
    let argc = 1 + args.len();
    let mut rec = (argc as u32).to_le_bytes().to_vec();
    rec.extend_from_slice(&(path.len() as u32).to_le_bytes());
    rec.extend_from_slice(path.as_bytes());
    for a in args {
        rec.extend_from_slice(&(a.len() as u32).to_le_bytes());
        rec.extend_from_slice(a.as_bytes());
    }
    let mut gr: Vec<u8> = Vec::with_capacity(grants.len() * 8);
    for (tag, h) in grants {
        gr.extend_from_slice(&tag.to_le_bytes());
        gr.extend_from_slice(&h.to_le_bytes());
    }
    let flags = if want_window { EXEC_REQUEST_WINDOW } else { 0 };
    let mut out = [0u32; 2];
    let r = syscall6(
        SYS_PROCESS_EXEC,
        rec.as_ptr() as u64,
        rec.len() as u64,
        gr.as_ptr() as u64,
        grants.len() as u64,
        out.as_mut_ptr() as u64,
        flags,
    );
    match r.ok() {
        Ok(tid) => Ok(Child { thread_id: tid as u32, proc_h: out[0], main_h: out[1] }),
        Err(st) => Err(st),
    }
}
```
Ensure `SYS_PROCESS_EXEC` and `EXEC_REQUEST_WINDOW` are imported (the file already does `use crate::syscall::*;`, which re-exports the abi syscall constants — verify `EXEC_REQUEST_WINDOW` is reachable; if `crate::syscall` doesn't re-export it, add `use abi::syscall::EXEC_REQUEST_WINDOW;`).

- [ ] **Step 2: Build the apps workspace**

Run: `cd apps && cargo build --release`
Expected: `Finished`.

- [ ] **Step 3: Commit**
```bash
git add apps/sdk/src/process.rs
git commit -m "sdk: process::exec — spawn by path with attested identity + window

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: sh — `run` uses `exec`

**Files:**
- Modify: `apps/shell/src/main.rs` (the `run` method, lines 112-140)

**Interfaces:**
- Consumes: `tinyos_app::process::exec` (Task 3).

- [ ] **Step 1: Rewrite `run` to exec by path**

`apps/shell/src/main.rs` — replace the top of `run` (the `fs::read` + `process::spawn`) so it execs the path and requests a window. Keep the foreground/background handling identical:
```rust
    fn run(&mut self, name: &str, args: &[&str], background: bool) {
        let grants = self.child_grants();
        let path = format!("/apps/{name}");
        match process::exec(&path, args, &grants, /*want_window=*/ true) {
            Ok(child) => {
                if background {
                    out(DIM, &format!("[{}] {name} &", child.thread_id));
                    self.jobs.push(Job { name: name.to_string(), child });
                } else {
                    if let Some(c) = entry::console() {
                        c.set_foreground(child.thread_id);
                    }
                    child.wait();
                    if let Some(c) = entry::console() {
                        c.set_foreground(0);
                        c.set_input_mode(INPUT_MODE_LINES);
                    }
                }
            }
            Err(st) => err(&format!("run: {name}: not found or failed (status {st})")),
        }
    }
```
Notes: `sh` no longer reads `/apps/{name}` (the kernel does), so the leading `fs::read` block is removed — and with it the ability to distinguish "not found" from other errors (exec returns `ST_INVALID_ARGS` for a missing/failed load). A single generic error arm is correct; do NOT match `abi::fs::FS_NOT_FOUND` (that's the fs-protocol status namespace, not the syscall status exec returns). `want_window = true` always; the kernel mints a window only for `window`-declaring apps. If the `FS_NOT_FOUND` import (line 17) becomes unused after removing the `fs::read`, drop it to avoid a warning.

- [ ] **Step 2: Build the apps workspace**

Run: `cd apps && cargo build --release`
Expected: `Finished`.

- [ ] **Step 3: Commit**
```bash
git add apps/shell/src/main.rs
git commit -m "sh: run apps via exec-by-path (kernel-attested identity + window)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Integration — attestation via `ps`, windows via manual check

**Files:**
- Modify: `tools/smoke/smoke.py`

**Interfaces:** Consumes the whole system (Tasks 1-4).

- [ ] **Step 1: Build the disk + confirm existing smoke passes**

Run: `make sync-apps && make smoke`
Expected: `smoke: PASS`. This already exercises exec heavily — every `run hello …`, `run top`, the reboot, etc. now go through `sys_process_exec`. If exec is wrong, `run` breaks and the harness fails. If it FAILS, capture output and STOP (report BLOCKED — do not patch to force a pass).

- [ ] **Step 2: Add an attestation assertion**

The identity is serial-visible via `ps`. `tools/smoke/smoke.py` — after the existing steps (a good spot: right after the `ps (full listing)` step, or add a fresh block before shutdown), add:
```python
        # Kernel-attested identity: a background windowed app's process name in
        # `ps` comes from the /apps basename the KERNEL loaded (not an argv[0]
        # claim). run pixels & then ps must list a process named "pixels".
        step("attested spawn", "run pixels &", "] pixels &")
        step("ps shows attested name", "ps", "pixels")
```
(`pixels` is a windowed app with no `declare_caps!` → legacy manifest `window=true`, so it also opens a window — framebuffer-only, verified manually below. `run pixels &` backgrounds it so `ps` runs while it's alive.)

- [ ] **Step 3: Run smoke with the new step**

Run: `make smoke`
Expected: `smoke: PASS`, including `[out] [<tid>] pixels &` and a `ps` line containing `pixels`, no panic.

- [ ] **Step 4: Manual verification checklist (for the human reviewer)**

`make run` → Ctrl+K → `uterm`. In the terminal: `run edit` — a separate top-level window titled **`edit`** opens (identity from `/apps/edit`), alongside `uterm`; `run pixels` — a **`pixels`** window opens. Confirm `vi`/`top` still render *inside* `uterm` (SP1b, unchanged). Confirm the window's chrome identity matches the app name (not spoofable by the shell).

- [ ] **Step 5: Final both-arch check + commit**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`.
```bash
git add tools/smoke/smoke.py
git commit -m "smoke: verify kernel-attested identity via ps (run pixels & -> ps)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-review notes

- **Spec coverage:** syscall + flag (Task 1), the exec handler with attestation + gated window-mint + grant-move reuse (Task 2), SDK `exec` (Task 3), `sh` migration (Task 4), attestation gate via `ps` + manual window check (Task 5). Identity-as-capability and jail-clean resolution are explicitly out of scope (SP3 / future).
- **Identity vs authority:** attestation = basename of the kernel-resolved path (Task 2 Step 2); grants moved unchanged via the shared `take_grants` helper (no new ambient authority). Window-mint gated by `EXEC_REQUEST_WINDOW && manifest.window`.
- **argv convention:** exec record `argv[0]` = path (kernel-only); child argv = `argv[1..]` (Task 2 parses `all[1..]`; Task 3 builds `[path, ...args]`) — apps see args unchanged.
- **Type consistency:** `exec(path, args, grants, want_window) -> Result<Child,u32>` (Task 3) matches `sh`'s call (Task 4); `take_grants` returns `Vec<(u32, Handle)>` consumed by both `sys_process_spawn` and `sys_process_exec`; `SYS_PROCESS_EXEC`/`EXEC_REQUEST_WINDOW` (Task 1) used in Tasks 2-3.
- **Regression guard:** `sys_process_spawn` still exists and now shares `take_grants`; `make smoke` (Task 5) exercises exec on every `run`, catching breakage.
