# Userspace Terminal as Boot Default — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the userspace terminal (`/apps/terminal`) the boot default on capable systems, keeping the in-kernel terminal as an unchanged fallback, with compositor respawn and a migrated smoke serial mirror.

**Architecture:** `Shell::new` launches the userspace terminal as the default window when aarch64 + `/apps/terminal` is readable, else opens the in-kernel `TerminalApp` (verbatim today's behavior). The compositor remembers the default's launch recipe and respawns it (rate-limited, falling back to the kernel terminal on a crash-loop). Because the userspace terminal's output no longer flows through `Terminal::out`, a new `SYS_DEBUG_MIRROR` syscall lets it echo console lines to serial as `[out] …` when smoke mode is on, preserving the harness contract.

**Tech Stack:** Rust `no_std` (kernel aarch64+x86_64 UEFI; apps aarch64-unknown-none), Python stdlib smoke harness driving QEMU over QMP.

## Global Constraints

- **Both kernel arches must compile:** `cargo check -p kernel --target aarch64-unknown-uefi` AND `--target x86_64-unknown-uefi` after every kernel change. x86_64 has no userspace (`AddrSpace::new()→None`); the boot-default flip MUST be a no-op there (it falls back to `TerminalApp`).
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Do not delete** the kernel terminal, `textview`, the `App` trait, `TerminalApp`, or `monitor` — this plan keeps them (they are the fallback / telemetry). Deletion is deferred behind an x86_64 userspace port (spec §Future work).
- **Keep `smoke::mirror` and `Terminal::out`'s call to it intact** — SP2 *adds* a second mirror source (the syscall), it does not move the existing one.
- **Next free syscall number is 17** (0–13, 15, 16 used; 14 reserved for a future thread_spawn — do not reuse 14).

---

### Task 1: `SYS_DEBUG_MIRROR` syscall (abi + kernel handler + SDK wrapper)

Adds a syscall the userspace terminal will call to echo a console line to serial as `[out] …`, gated by the existing smoke flag (kernel-side no-op when off). Not independently unit-testable (kernel is `no_std` with no syscall host harness); verified by build here and end-to-end by the smoke gate in Task 7.

**Files:**
- Modify: `crates/abi/src/syscall.rs` (add the const near the other `SYS_*`)
- Modify: `kernel/src/obj/syscall.rs` (dispatch arm + handler + import)
- Create: `apps/sdk/src/debug.rs`
- Modify: `apps/sdk/src/lib.rs` (declare `pub mod debug;`)

**Interfaces:**
- Produces: `abi::syscall::SYS_DEBUG_MIRROR: u64 = 17`; kernel `sys_debug_mirror(buf: u64, len: u64) -> SysResult`; SDK `tinyos_app::debug::mirror(s: &str)`.

- [ ] **Step 1: Add the syscall number.** In `crates/abi/src/syscall.rs`, after the line `pub const SYS_MEMOBJ_UNMAP: u64 = 15;` add:

```rust
/// Echo a string to serial as `[out] …` when the smoke-test mirror is on
/// (a kernel-side no-op otherwise). Lets the userspace terminal feed the
/// headless harness the same output the in-kernel terminal used to mirror.
pub const SYS_DEBUG_MIRROR: u64 = 17;
```

- [ ] **Step 2: Add the kernel handler.** In `kernel/src/obj/syscall.rs`, add after `sys_log` (right after its closing `}` near line 144):

```rust
fn sys_debug_mirror(buf: u64, len: u64) -> SysResult {
    if len > LOG_MAX {
        return Err(ST_INVALID_ARGS);
    }
    let bytes = copy_in(buf, len)?;
    match core::str::from_utf8(&bytes) {
        Ok(s) => {
            crate::smoke::mirror(s);
            Ok(0)
        }
        Err(_) => Err(ST_INVALID_ARGS),
    }
}
```

- [ ] **Step 3: Wire the dispatch arm + import.** In `kernel/src/obj/syscall.rs`, add `SYS_DEBUG_MIRROR` to the `use abi::syscall::{…}` list (keep it alphabetical: it sorts before `SYS_HANDLE_CLOSE`), and add this arm to the `match sysno` in `dispatch` (near the `SYS_LOG` arm):

```rust
        SYS_DEBUG_MIRROR => sys_debug_mirror(args[0], args[1]),
```

- [ ] **Step 4: Add the SDK wrapper.** Create `apps/sdk/src/debug.rs`:

```rust
//! Debug/serial helpers. `mirror` echoes a line to the serial port as
//! `[out] …` when the kernel's smoke-test mirror is active (a no-op
//! otherwise), so the userspace terminal can feed the headless harness the
//! console output it renders.

use crate::syscall::{syscall2, SYS_DEBUG_MIRROR};

/// Mirror one console line to serial (`[out] …`). Cheap no-op unless the
/// kernel booted with the smoke fw_cfg flag set.
pub fn mirror(s: &str) {
    let _ = syscall2(SYS_DEBUG_MIRROR, s.as_ptr() as u64, s.len() as u64);
}
```

- [ ] **Step 5: Export the module.** In `apps/sdk/src/lib.rs`, add alongside the other `pub mod` declarations:

```rust
pub mod debug;
```

- [ ] **Step 6: Verify both kernel arches + SDK build.**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished` (the pre-existing `glyph` dead-code warning is fine).
Run: `cd apps && cargo build -p tinyos-app --release --target aarch64-unknown-none; cd ..`
Expected: `Finished`.

- [ ] **Step 7: Commit.**

```bash
git add crates/abi/src/syscall.rs kernel/src/obj/syscall.rs apps/sdk/src/debug.rs apps/sdk/src/lib.rs
git commit -m "syscall: add SYS_DEBUG_MIRROR for the userspace terminal's serial mirror

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: termcore records frozen scrollback lines for mirroring (TDD)

`termcore` is a pure, host-tested model — the one place real TDD applies. It gains a drain-once buffer of the lines it freezes, so the app layer can mirror them without re-parsing the console protocol.

**Files:**
- Modify: `crates/termcore/src/lib.rs` (field + `push_line` + `take_mirror` + a test)

**Interfaces:**
- Consumes: nothing new.
- Produces: `Term::take_mirror(&mut self) -> Vec<String>` — the texts of scrollback lines frozen since the previous call, in order.

- [ ] **Step 1: Write the failing test.** In `crates/termcore/src/lib.rs`, inside the existing `#[cfg(test)] mod tests { … }`, add (model the message on the existing `OP_WRITE` test):

```rust
    #[test]
    fn take_mirror_returns_frozen_lines_then_drains() {
        let mut t = Term::new();
        // Two complete lines in one OP_WRITE payload.
        let mut b = abi::console::OP_WRITE.to_le_bytes().to_vec();
        b.extend_from_slice(b"alpha\nbeta\n");
        t.on_console_msg(&b);
        assert_eq!(t.take_mirror(), alloc::vec!["alpha".to_string(), "beta".to_string()]);
        // Draining is one-shot: a second call is empty until more freezes.
        assert!(t.take_mirror().is_empty());
    }
```

- [ ] **Step 2: Run it and confirm it fails.**

Run: `cd apps 2>/dev/null; cd ..; cargo test -p termcore --lib take_mirror 2>&1 | tail -20`
Expected: FAIL — `no method named take_mirror found for struct Term`.

- [ ] **Step 3: Add the field.** In `crates/termcore/src/lib.rs`, in the `Term` struct definition (near `scrollback: VecDeque<Line>,`), add:

```rust
    /// Texts of lines frozen since the last `take_mirror` drain. Used only to
    /// echo output to serial in smoke runs; drained every frame by the app so
    /// it never accumulates.
    mirror: Vec<String>,
```

And in `Term::new()`'s struct literal (near `scrollback: VecDeque::new(),`), add:

```rust
            mirror: Vec::new(),
```

- [ ] **Step 4: Record frozen lines in `push_line`.** In `push_line` (the method doing `self.scrollback.push_back(Line { text, color });`), record the text before it moves into the `Line`:

```rust
    fn push_line(&mut self, text: String, color: u32) {
        self.mirror.push(text.clone());
        self.scrollback.push_back(Line { text, color });
        while self.scrollback.len() > SCROLLBACK_CAP {
            self.scrollback.pop_front();
        }
    }
```

- [ ] **Step 5: Add `take_mirror`.** Add near the other public accessors (e.g. after `take_outbound`):

```rust
    /// Drain the texts of lines frozen since the last call (for the serial
    /// mirror). One-shot: subsequent calls are empty until more lines freeze.
    pub fn take_mirror(&mut self) -> Vec<String> {
        core::mem::take(&mut self.mirror)
    }
```

- [ ] **Step 6: Run the test and confirm it passes (and no regressions).**

Run: `cargo test -p termcore --lib 2>&1 | tail -20`
Expected: PASS, all termcore tests green.

- [ ] **Step 7: Commit.**

```bash
git add crates/termcore/src/lib.rs
git commit -m "termcore: expose frozen scrollback lines via take_mirror

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: userspace terminal mirrors console lines to serial

Drains `take_mirror` each loop and calls the new syscall. Verified by build here; end-to-end by Task 7.

**Files:**
- Modify: `apps/terminal/src/main.rs` (drain + mirror in the main loop)

**Interfaces:**
- Consumes: `Term::take_mirror` (Task 2), `tinyos_app::debug::mirror` (Task 1).

- [ ] **Step 1: Mirror frozen lines each loop.** In `apps/terminal/src/main.rs`, immediately after the `while let Ok(msg) = con_kern.try_recv() { … }` console-drain loop closes (right after the block ending near line 215, before the `if term.surface().is_none()` check), add:

```rust
        // Echo any newly-frozen scrollback lines to serial for the smoke
        // harness. `debug::mirror` is a kernel no-op unless smoke mode is on,
        // so this costs one syscall per output line only in headless runs.
        for line in term.take_mirror() {
            tinyos_app::debug::mirror(&line);
        }
```

- [ ] **Step 2: Verify the app builds.**

Run: `cd apps && cargo build -p terminal --release --target aarch64-unknown-none 2>&1 | tail -5; cd ..`
Expected: `Finished`.

- [ ] **Step 3: Commit.**

```bash
git add apps/terminal/src/main.rs
git commit -m "uterm: mirror console output to serial (SYS_DEBUG_MIRROR)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: `launch_uterm` returns success + `is_default` plumbing

Makes the launch report whether it actually spawned (so the boot path can fall back), and tags the boot-default window so the reaper can recognize it. Verified by both-arch build.

**Files:**
- Modify: `kernel/src/ui/shell/extern_app.rs` (`PendingApp.is_default`, `ExternApp.is_default` + accessor, `register_default`, `try_open` copies the flag)
- Modify: `kernel/src/ui/shell/mod.rs` (`launch_uterm(as_default: bool) -> bool`; update the palette call site)

**Interfaces:**
- Produces: `extern_app::register_default(shell: Arc<ChannelEnd>, name: String)`; `ExternApp::is_default(&self) -> bool`; `Shell::launch_uterm(&mut self, as_default: bool) -> bool`.

- [ ] **Step 1: Add `is_default` to `PendingApp` + `register_default`.** In `kernel/src/ui/shell/extern_app.rs`, add the field to `PendingApp` (after `focus_on_open`):

```rust
    /// This is the compositor's boot-default terminal — the reaper respawns it
    /// when it exits (ordinary windows are reaped silently).
    pub is_default: bool,
```

Set it in the existing `register` (which pushes `is_default: false`) and add a sibling:

```rust
pub fn register(shell: Arc<ChannelEnd>, name: String, focus_on_open: bool) {
    SPAWN_QUEUE.lock().push(PendingApp {
        shell,
        name,
        focus_on_open,
        is_default: false,
    });
}

/// Register the boot-default terminal: focused, and tagged so the compositor
/// respawns it if it exits.
pub fn register_default(shell: Arc<ChannelEnd>, name: String) {
    SPAWN_QUEUE.lock().push(PendingApp {
        shell,
        name,
        focus_on_open: true,
        is_default: true,
    });
}
```

- [ ] **Step 2: Carry the flag into `ExternApp`.** In `extern_app.rs`, add to the `ExternApp` struct (near `name: String,`):

```rust
    /// True for the compositor's boot-default terminal (drives respawn).
    is_default: bool,
```

In `try_open`, in the `OpenResult::Opened(ExternApp { … })` literal, add:

```rust
                is_default: pending.is_default,
```

And add an accessor (near the other `impl ExternApp` methods):

```rust
    pub fn is_default(&self) -> bool {
        self.is_default
    }
```

- [ ] **Step 3: `launch_uterm` returns bool + `as_default`.** In `kernel/src/ui/shell/mod.rs`, change the aarch64 `launch_uterm` signature and body tail:

```rust
    #[cfg(target_arch = "aarch64")]
    pub fn launch_uterm(&mut self, as_default: bool) -> bool {
        use crate::obj::channel::create;
        use crate::obj::handle::{Handle, RIGHTS_ALL};
        use crate::obj::Object;
        use abi::bootstrap::{TAG_FS, TAG_FS_BROKER, TAG_PROC, TAG_PROC_BROKER, TAG_SHELL};
        let elf = match crate::fs::read("/", "/apps/terminal") {
            Ok(e) => e,
            Err(e) => { kprintln!("uterm: /apps/terminal: {e}"); return false; }
        };
        let (shell_app, shell_kern) = create();
        let grants = alloc::vec![
            (TAG_SHELL, Handle::new(Object::Channel(shell_app), RIGHTS_ALL)),
            (TAG_FS, crate::svc::mint_fs()),
            (TAG_PROC, crate::svc::mint_proc()),
            (TAG_FS_BROKER, crate::svc::fs_broker_handle()),
            (TAG_PROC_BROKER, crate::svc::proc_broker_handle()),
        ];
        match crate::obj::loader::spawn_with_grants("terminal".into(), &elf, &[], grants) {
            Ok((_p, tid, _main)) => {
                if as_default {
                    crate::ui::shell::extern_app::register_default(shell_kern, "Terminal".into());
                } else {
                    crate::ui::shell::extern_app::register(shell_kern, "Terminal".into(), true);
                }
                kprintln!("tinyos: uterm launched (thread {tid})");
                true
            }
            Err(e) => { kprintln!("uterm: spawn failed: {}", e.msg()); false }
        }
    }

    #[cfg(not(target_arch = "aarch64"))]
    pub fn launch_uterm(&mut self, _as_default: bool) -> bool {
        kprintln!("uterm: userspace unsupported on this arch");
        false
    }
```

- [ ] **Step 4: Update the palette call site.** In `mod.rs`, in `open_named`, change `"uterm" => self.launch_uterm(),` to:

```rust
            "uterm" => { self.launch_uterm(false); }
```

- [ ] **Step 5: Verify both arches build.**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`.

- [ ] **Step 6: Commit.**

```bash
git add kernel/src/ui/shell/extern_app.rs kernel/src/ui/shell/mod.rs
git commit -m "shell: launch_uterm reports success + tags the boot-default window

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: boot default = userspace terminal, with fallback

Flips `Shell::new` to launch uterm as the default when possible, else open the kernel `TerminalApp`. On x86_64 `launch_uterm` returns false → fallback (true no-op for that arch).

**Files:**
- Modify: `kernel/src/ui/shell/mod.rs` (`Shell::new` tail)

**Interfaces:**
- Consumes: `Shell::launch_uterm(as_default=true) -> bool` (Task 4).

- [ ] **Step 1: Flip the boot default.** In `kernel/src/ui/shell/mod.rs`, replace the single line in `Shell::new`:

```rust
        shell.open(Box::new(crate::apps::terminal::TerminalApp::new()), true);
        shell
```

with:

```rust
        // Boot into the userspace terminal where it can run (aarch64 with
        // /apps/terminal present); otherwise fall back to the in-kernel
        // terminal — the only shell on x86_64 or a diskless boot. The window
        // appears a few frames after the splash while the app execs and opens
        // it (vs the kernel terminal's synchronous window).
        if !shell.launch_uterm(true) {
            shell.open(Box::new(crate::apps::terminal::TerminalApp::new()), true);
        }
        shell
```

- [ ] **Step 2: Verify both arches build.**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`.

- [ ] **Step 3: Commit.**

```bash
git add kernel/src/ui/shell/mod.rs
git commit -m "shell: boot into the userspace terminal, fall back to the kernel one

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: compositor respawn of the default terminal

When the boot-default uterm window is reaped, respawn it — rate-limited, giving up to the kernel terminal on a crash-loop so the desktop always has a shell.

**Files:**
- Modify: `kernel/src/ui/shell/mod.rs` (a `DefaultTerm` record + `Shell` field + set it in `launch_uterm` success + a `respawn_default_terminal` method + hook the `pump_externals` reap)

**Interfaces:**
- Consumes: `ExternApp::is_default` (Task 4), `Shell::launch_uterm` (Task 4).

- [ ] **Step 1: Add the respawn record type.** In `kernel/src/ui/shell/mod.rs`, near the top-level `struct`/`const` declarations (e.g. above `pub struct Shell`), add:

```rust
/// How soon after launch a default-terminal exit counts as a crash (µs).
const DEFAULT_TERM_FAST_CRASH_US: u64 = 3_000_000;
/// Consecutive fast crashes before giving up on the userspace terminal.
const DEFAULT_TERM_MAX_FAST_CRASHES: u32 = 3;

/// Respawn bookkeeping for the boot-default userspace terminal. `None` when the
/// default is the in-kernel fallback (never exits) or respawn has given up.
struct DefaultTerm {
    /// Uptime (µs) when the current default terminal was launched.
    launched_us: u64,
    /// Consecutive exits within `DEFAULT_TERM_FAST_CRASH_US` of launch.
    fast_crashes: u32,
}
```

- [ ] **Step 2: Add the `Shell` field.** In the `Shell` struct (after `svc_jobs: Vec<svc::SvcJob>,`), add:

```rust
    /// Set when the boot default is the userspace terminal; drives respawn.
    default_term: Option<DefaultTerm>,
```

And in `Shell::new`'s struct literal (after `svc_jobs: Vec::new(),`), add:

```rust
            default_term: None,
```

- [ ] **Step 3: Record the launch in `launch_uterm`.** In the aarch64 `launch_uterm`, inside the `Ok((_p, tid, _main))` arm, in the `if as_default { … }` branch, after the `register_default(...)` call add the bookkeeping (preserving any accumulated crash count across respawns):

```rust
                if as_default {
                    crate::ui::shell::extern_app::register_default(shell_kern, "Terminal".into());
                    let fast = self.default_term.as_ref().map_or(0, |d| d.fast_crashes);
                    self.default_term = Some(DefaultTerm {
                        launched_us: crate::arch::timer::uptime_us(),
                        fast_crashes: fast,
                    });
                } else {
                    crate::ui::shell::extern_app::register(shell_kern, "Terminal".into(), true);
                }
```

- [ ] **Step 4: Add the respawn method.** In `impl Shell`, near `pump_externals`, add:

```rust
    /// The boot-default terminal window exited. Respawn it, unless it has been
    /// crash-looping — then fall back to the in-kernel terminal so the desktop
    /// always has a working shell rather than a spinning respawn.
    fn respawn_default_terminal(&mut self) {
        let now = crate::arch::timer::uptime_us();
        let fast = if let Some(dt) = self.default_term.as_mut() {
            if now.saturating_sub(dt.launched_us) < DEFAULT_TERM_FAST_CRASH_US {
                dt.fast_crashes += 1;
            } else {
                dt.fast_crashes = 0;
            }
            dt.fast_crashes
        } else {
            return; // default is the kernel terminal (or gave up): nothing to do
        };
        if fast >= DEFAULT_TERM_MAX_FAST_CRASHES {
            kprintln!("tinyos: userspace terminal crash-looping — falling back to kernel terminal");
            self.default_term = None;
            self.open(Box::new(crate::apps::terminal::TerminalApp::new()), true);
        } else {
            kprintln!("tinyos: userspace terminal exited — respawning");
            self.launch_uterm(true); // preserves fast_crashes via the record above
        }
    }
```

- [ ] **Step 5: Hook the reap loop.** In `pump_externals`, replace the reap `while` loop (the `let mut i = 0; while i < self.windows.len() { … }` block) with one that notices a default-terminal exit:

```rust
        let mut i = 0;
        while i < self.windows.len() {
            let (closed, was_default) = match self.windows[i]
                .app
                .as_any()
                .downcast_mut::<extern_app::ExternApp>()
            {
                Some(a) => (a.pump(), a.is_default()),
                None => (false, false),
            };
            if closed {
                self.windows.remove(i);
                self.focus_topmost_visible();
                if was_default {
                    self.respawn_default_terminal();
                }
            } else {
                i += 1;
            }
        }
```

- [ ] **Step 6: Verify both arches build.**

Run: `cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: both `Finished`. (On x86_64 `default_term` stays `None` — `launch_uterm` returns false, so the code is inert but must compile; `DefaultTerm`/consts are used by the aarch64 path and the shared `respawn_default_terminal`.)

- [ ] **Step 7: Commit.**

```bash
git add kernel/src/ui/shell/mod.rs
git commit -m "shell: respawn the boot-default terminal, fall back on crash-loop

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: userspace terminal exits when its shell exits

Realizes the spec's "typed `exit` respawns" case and gives the smoke a reliable keyboard trigger for the respawn test. `sh` already has an `exit`/`logout` builtin (`apps/shell/src/main.rs:182`, `return false` ends its REPL → `sh` process exits). But the terminal keeps running with a dead console when `sh` goes — it should exit so the compositor reaps it (→ respawn for the default; plain close for a palette instance). `sh` closing the console is the *only* thing that peer-closes `con_kern` (child dups of the console close first while `sh` holds the original), so this triggers only on shell exit, never when a hosted app like `top` quits.

**Files:**
- Modify: `apps/terminal/src/main.rs` (console-drain loop detects peer-close)

**Interfaces:**
- Consumes: `tinyos_app::syscall::ST_SHOULD_WAIT` (already in the SDK).

- [ ] **Step 1: Detect the shell exiting.** In `apps/terminal/src/main.rs`, replace the console-drain loop header `while let Ok(msg) = con_kern.try_recv() {` with a form that distinguishes "drained" from "peer gone":

```rust
        loop {
            let msg = match con_kern.try_recv() {
                Ok(m) => m,
                Err(tinyos_app::syscall::ST_SHOULD_WAIT) => break, // console drained this frame
                // Any other error means sh (the console peer) is gone — the
                // shell exited (`exit`/`logout`) or crashed. Close the terminal;
                // the compositor respawns it if it was the boot default.
                Err(_) => { close = true; break; }
            };
```

The existing loop body (the `OP_SURFACE_OPEN`/`OP_SURFACE_CLOSE` handling and `term.on_console_msg`) and its closing `}` are unchanged — only the header line changes from `while let Ok(msg) = …` to the `loop { let msg = match … }` above.

- [ ] **Step 2: Break the main loop on shell exit.** Immediately after the mirror-drain loop added in Task 3 (`for line in term.take_mirror() { … }`), add:

```rust
        if close {
            break;
        }
```

(`close` is the per-iteration bool already set by `Event::CloseRequested`; this reuses it so window-close and shell-exit share one exit path.)

- [ ] **Step 3: Verify the app builds.**

Run: `cd apps && cargo build -p terminal --release --target aarch64-unknown-none 2>&1 | tail -5; cd ..`
Expected: `Finished`.

- [ ] **Step 4: Commit.**

```bash
git add apps/terminal/src/main.rs
git commit -m "uterm: exit when the hosted shell exits (enables respawn on \`exit\`)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: update the smoke harness + full integration gate

With uterm as the boot default, the early smoke steps now drive the userspace terminal (output arrives via the new mirror). Remove the now-redundant palette-launch-uterm step, add a respawn assertion, and prove the whole thing end to end.

**Files:**
- Modify: `tools/smoke/smoke.py` (retarget the uterm-launch section; add a respawn step)

**Interfaces:**
- Consumes: everything above (this is the integration gate).

- [ ] **Step 1: Read the current uterm section.** Open `tools/smoke/smoke.py` and locate the block commented `# 11. Launch path: the command palette (Ctrl+K) -> uterm` through the `# 12. Durability` comment (the palette `Ctrl+K`/`uterm` launch and the `(in uterm) run top` step). This block assumed the default was the kernel terminal and uterm was launched on demand — no longer true.

- [ ] **Step 2: Replace the palette-launch block with a respawn test.** Replace that block (from the `# 11.` comment up to, but not including, the `# 12. Durability` comment) with:

```python
        # 11. Respawn: the boot default IS the userspace terminal now. `exit`
        #     ends sh (apps/shell/src/main.rs:182); the terminal then exits
        #     (Task 7) and the compositor must respawn a fresh one so the
        #     desktop is never left shell-less. Assert output round-trips again.
        print("smoke: > exit    (end sh -> terminal exits -> compositor respawns it)")
        qmp.type_line("exit")
        cur = serial.wait_for("userspace terminal exited — respawning", args.step_timeout, cur)
        cur = serial.wait_for("tinyos: uterm launched", args.step_timeout, cur)
        time.sleep(0.8)                          # let the respawned terminal spawn sh
        step("respawned terminal is live", "echo respawned", "[out] respawned")
```

- [ ] **Step 3: Confirm the boot markers still fire.** Verify the smoke's boot wait sequence (near the top of `main`) still matches: `tinyos: shell up` is a kernel print (unchanged) and `[out] tinyOS shell` now arrives via the new mirror after uterm+sh start. No change expected; if the `[out] tinyOS shell` wait times out, raise `boot_timeout` for that wait (uterm+sh start later than the in-kernel terminal did).

- [ ] **Step 4: Run the full smoke gate.**

Run: `pgrep -fl qemu-system-aarch64 || make smoke 2>&1 | tail -30`
Expected: `smoke: PASS`. If a stray QEMU holds `disk.img`, stop (a `make run` window is open) and coordinate before killing it.

- [ ] **Step 5: Both-arch build gate.**

Run: `make test 2>&1 | grep -iE "error|test result: FAIL" ; cargo check -p kernel --target aarch64-unknown-uefi && cargo check -p kernel --target x86_64-unknown-uefi`
Expected: no errors; both arches `Finished`; all host suites pass.

- [ ] **Step 6: Commit.**

```bash
git add tools/smoke/smoke.py
git commit -m "smoke: drive the userspace terminal as the boot default + test respawn

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Manual verification (after Task 8, before merge)

Boot-critical + visual — the user runs these in QEMU (`make sync-apps && make run` on aarch64):
- Desktop comes up in the **userspace terminal** (chrome shows the `>_` icon + "Terminal"); after the splash there is a brief moment before the window appears.
- Type `exit` (or close the terminal window) → a fresh terminal **respawns**.
- Palette/dock, `monitor`, and windowed apps (`run pixels`, `run edit`) still work; `run top`/`vi` still render inside the terminal.
- Sanity: the kernel-terminal **fallback** is unchanged — not exercised here (needs a diskless/x86_64 boot), noted as a known gap in the spec.

## Notes for the implementer

- **The mirror is always-call / kernel-no-op** (resolves the spec's open question): the userspace terminal calls `SYS_DEBUG_MIRROR` for every frozen line; the kernel drops it unless the smoke fw_cfg flag is set. One syscall per output line only matters in headless runs; console output is human-rate, so the cost is negligible and there is no per-app "am I in smoke mode" query to plumb.
- **Rate-limit numbers** (`DEFAULT_TERM_FAST_CRASH_US = 3s`, `MAX_FAST_CRASHES = 3`) are deliberate, conservative defaults; the give-up path logs and falls back to the kernel terminal.
- **Do not** touch `smoke::mirror` / `Terminal::out` — the kernel terminal still mirrors on the fallback path.
