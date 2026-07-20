# tinyOS Developer Ergonomics — Assessment & Roadmap

*2026-07-20*

## Context

tinyOS has an explicit goal of supporting **third-party app authorship** (`docs/superpowers/specs/2026-07-18-app-api-design.md:7`), and the foundations are strong: a tiny (~16) capability syscall ABI, a single source-of-truth `crates/abi`, an SDK that hides the capability machinery behind `fn main(env: Env) -> i32`, and a fast iteration loop (`make sync-apps`, no OS rebuild). Yet nothing pulls the seams together into a low-friction authoring story, and the immediate-mode GUI path still makes every app hand-roll its own event loop.

This document evaluates the current interface/programming model against the goal *"as easy as possible to build an app"* and proposes a **sequenced roadmap**. Direction agreed:

- **Deliverable:** assessment + prioritized roadmap. No code with this doc — the roadmap is the artifact; each wave later gets its own spec → plan → implementation cycle.
- **Audience:** design toward a genuine third-party end state, but **sequence first-party velocity first**.
- **Language ambition:** stay Rust `no_std`, and add a **higher-level (retained-mode) app framework** on top of the existing immediate-mode primitives.

The intended outcome is a shared picture of *where the ergonomic friction is* and *what order to fix it in* — so the next build sessions can be picked off this doc.

## Current state — what's good, what hurts

**Strong foundations (keep, don't disturb):**

- Coherent philosophy: no POSIX, "all new kernel APIs are handle/channel-shaped" (`roadmap.md:36`). Ergonomic wins should require **zero new syscalls** — every item below is userspace/SDK/build-tooling only.
- `hello` is genuinely ~8 lines; `println!`/`read_line()` work with no ceremony (`apps/hello/src/main.rs`).
- Single ABI source of truth in `crates/abi`; design tokens shared with the compositor so UI can't drift (`crates/abi/src/tokens.rs`).
- Host-testable pure engines (`vicore`, `tinyfs`, `textui`) already get `cargo test`.

**Friction points (ranked by leverage):**

1. **Registration is triplicated.** A new app edits three files: `apps/<name>/Cargo.toml`, `apps/Cargo.toml` `members`, and `Makefile` `APP_BINS`. No scaffold generator. Easy to forget, pure boilerplate.
2. **Capabilities are a stringly-typed blob.** `declare_caps!(b"console\nwindow\nfs:self")` (`apps/sdk/src/lib.rs:69`) — typos fail silently, no valid-token discoverability, no compile-time link to the `Env` field each cap unlocks.
3. **Immediate-mode boilerplate per GUI app.** Every window app hand-rolls open→`pixels()`→draw→`present()`→`poll_events()`→`wait()` plus `UiInput::begin_frame`/`feed` bookkeeping (`apps/pixels`, `apps/clock`, `ui.rs`). Fine as a floor; punishing as the only option.
4. **No app identity/manifest.** Name, display name, version, icon, launcher category are implicit or hardcoded in the shell. Blocks any real launcher/dock/install metadata story.
5. **New services are hand-rolled twice.** Each service = a hand-written opcode enum in `crates/abi` + a hand-written client in the SDK (fs/proc/window/console). Ripe for define-once codegen.
6. **Testing stops at the door.** Pure engines are host-testable, but there's no harness to run a *whole app* (its loop, window, events) against a mock `Env` without booting QEMU.
7. **No lifecycle/packaging & no authoring docs.** Apps are ELF blobs baked into `disk.img`; `install` exists but there's no package format, ABI-compat metadata, or authoring guide.

## Guardrails (so this stays "no bloat")

- **Zero new syscalls.** The 16-syscall surface is frozen; all wins are SDK / build-tooling / userspace protocol.
- **Additive and layered.** The raw capability ABI, `gfx::Canvas`, and immediate-mode `ui` all stay and stay public. The framework sits *on top* with a documented escape hatch (drop to `Canvas` inside `view`, or drop to raw `Env` entirely).
- **Single source of truth.** New schemas (cap tokens, manifest fields) live in `crates/abi` so kernel loader, shell/launcher, SDK, and host tools can't drift — same discipline as `tokens.rs`.
- **Crate-hood earned.** Per `terminal-and-crates-design.md:34`, a new crate only when it has ≥2 consumers or needs host-testability.
- **YAGNI.** Packaging/signing/app-store deferred to the last wave; nothing speculative before then.

## Roadmap

### Wave A — First-party velocity (low risk, high daily payoff)

**A1. Kill the triple-registration.** Derive the app list from the filesystem instead of three hand-maintained lists. Options to evaluate in A1's own spec: glob `members` in `apps/Cargo.toml`, and a `build.rs`/make snippet that discovers `APP_BINS` from `apps/*/Cargo.toml`. Add a `tinyos new <name>` (or `make new-app NAME=…`) scaffold that emits the `Cargo.toml` + `main.rs` boilerplate pre-wired. *Acceptance:* adding an app touches exactly one directory; `make sync-apps` picks it up with no other edits.

**A2. Typed capabilities.** Replace the byte-blob with typed constants defined once in `crates/abi` and re-exported by the SDK, e.g. `declare_caps!(Cap::Console, Cap::Window, Cap::FsSelf)` / `Cap::shared("logs")`. The macro still emits the exact same `.tinyos_abi.caps` bytes the loader already parses (`kernel/src/obj/loader.rs`) — wire-compatible, so no kernel change. Benefits: compile-time validation, IDE discoverability, and a natural place to document what `Env` field each cap unlocks. *Acceptance:* a typo is a compile error; the emitted blob is byte-identical to today's for the same caps.

**A3. Host app harness.** A `MockEnv` (in-memory console/fs/window channels) so an app's logic can be driven under `cargo test` on the host, including snapshot-rendering a window's BGRA buffer to PNG for golden tests. Pairs with the framework (B1): a retained-mode `App` is trivially testable by feeding events and asserting on `view` output. *Acceptance:* at least one existing app (e.g. `solitaire`, which already has a host-tested core) gains an end-to-end host test that exercises its event loop.

### Wave B — The framework + app identity (the headline change)

**B1. Retained-mode app framework.** Add an `App` trait + SDK-owned runtime that owns the open→present→poll→wait loop, damage tracking, and event dispatch, layered over the existing `Window`/`gfx`/`ui`. Sketch:

```rust
struct Counter { n: i32 }
impl tinyos_app::App for Counter {
    fn view(&self, ui: &mut Ui) {                 // retained UI; SDK owns the loop
        ui.label(&format!("{}", self.n));
        if ui.button("+") { /* handled below */ }
    }
    fn on_event(&mut self, ev: Event) { /* update state */ }
}
tinyos_app::run_app!(Counter { n: 0 });           // replaces app!(main) + hand-rolled loop
```

The runtime handles `CloseRequested`, redraw-on-change, and frame pacing. Console apps get a parallel simplification (a `run` helper for the common read-line-loop shape). **Immediate mode stays public** — `view` hands out a `Canvas` escape hatch, and `app!(main)` remains for apps that want the raw loop. *Acceptance:* `pixels`/`clock` re-expressed in the framework are meaningfully shorter with no lost capability; the raw path still compiles and runs.

**B2. App manifest / identity.** A declarative manifest (name, display name, version, icon handle, launcher category, + the caps from A2) emitted into an ELF section the loader and shell/launcher read — schema defined once in `crates/abi`. Likely a single macro or a `[package.metadata.tinyos]` block consumed at build time. Unlocks a real launcher/dock/install-metadata story and is the foundation Wave C builds on. *Acceptance:* the launcher/dock renders name + icon from the binary itself rather than a hardcoded table.

### Wave C — Third-party end state (deferred; needs B done first)

**C1. Protocol codegen.** A `service!`-style macro that defines a service's opcodes + message types once and generates both the `crates/abi` enum and the SDK client, cutting the "hand-rolled twice" cost for future services. *Acceptance:* one existing service (e.g. `proc`) is regenerated from a single definition with no behavior change.

**C2. Packaging & lifecycle.** A package format bundling the ELF + manifest (B2) + icon, an `install`/`uninstall`/list flow, and an ABI-compat check on install. Explicitly YAGNI until B2 lands and there's a second consumer. *Acceptance:* an app installs from a single package artifact and appears in the launcher without editing the build.

**C3. Authoring docs & examples.** An app-authoring guide (the current gap), an indexed examples set (`hello` → console-input → framework GUI → service client), and a caps reference generated from the A2 typed tokens. *Acceptance:* someone outside the repo can build and run a GUI app from the guide alone.

## Sequencing

- **A** can proceed in any order and in parallel — none of A1/A2/A3 depend on each other.
- **B1 and B2 pair naturally** (identity + framework entry point) and follow Wave A.
- **C must not start before B2 exists**, since packaging, codegen consumers, and docs all build on the manifest schema and framework.

Each wave item carries its own acceptance criteria so it is independently actionable when picked up. The next step is to choose the first item and turn it into its own spec via the `writing-plans` flow.
