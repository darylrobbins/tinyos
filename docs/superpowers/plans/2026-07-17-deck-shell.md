# Deck Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace tinyOS's macOS-style desktop with Deck — a command-first card shell with an instrument-panel identity — per `docs/superpowers/specs/2026-07-17-deck-shell-design.md`.

**Architecture:** A `Card` trait renders apps into lane-packed rects owned by a `Deck` struct that also owns the command bar, focus, status strip, and animation clock. Apps live in `kernel/src/apps/`. The old `ui/desktop.rs` + wallpaper are deleted at the end. Everything below the shell (kernel, drivers, gfx) is untouched.

**Tech Stack:** existing tinyOS kernel (Rust nightly, no_std, `aarch64-unknown-uefi` + `x86_64-unknown-uefi`), fontdue, QEMU/QMP for verification.

## Global Constraints

- Tokens (spec, verbatim): field `#17181A`, dot grid `#26282B` 24px pitch, panel `#232528`, panel-hi `#2A2D30`, hairline `#3A3E42`, ink `#E8EAEC`, ink-dim `#9BA1A6`, signal `#FF6A00` (live/active only).
- 4px card radius; no blur/gradients/glass; motion 100–120ms linear.
- Small-caps labels = uppercase Inter + letter spacing (draw per-char with +1px advance is acceptable).
- Keyboard: `Ctrl+K` bar toggle, `Esc` dismiss, `Ctrl+arrows` focus move; Ctrl keycode = 29.
- Test cycle per task: `make build 2>&1 | grep -E '^error' -A6` must print nothing, then the QMP boot script below where specified. Headless QEMU invocation pattern (aarch64) is the one used throughout this repo's history:
  `qemu-system-aarch64 -machine virt -m 512M -accel hvf -cpu host -drive if=pflash,format=raw,readonly=on,file=build/code-aarch64.fd -drive if=pflash,format=raw,file=build/vars-aarch64.fd -device ramfb -device virtio-keyboard-pci -device virtio-tablet-pci -drive format=raw,file=fat:rw:esp -fw_cfg name=opt/tinyos/res,string=1440x900 -display none -serial file:/tmp/tinyos-serial.log -qmp unix:/tmp/tinyos-qmp.sock,server,nowait -monitor none`
  (run `make firmware` first; screenshots via QMP `screendump` → `sips -s format png` → Read the png).
- Commit after every task with the shown message.

---

### Task 1: Tokens, Card trait, Deck scaffold with field + status strip

**Files:**
- Create: `kernel/src/ui/deck/mod.rs`, `kernel/src/ui/deck/tokens.rs`, `kernel/src/ui/deck/card.rs`
- Modify: `kernel/src/ui/mod.rs` (add `pub mod deck;`), `kernel/src/main.rs` (swap desktop loop for deck loop)

**Interfaces:**
- Produces `tokens`: `pub const FIELD/GRID/PANEL/PANEL_HI/HAIRLINE/INK/INK_DIM/SIGNAL: u32` (values from Global Constraints, via `gfx::surface::rgb`).
- Produces `card.rs`:
  ```rust
  pub trait Card {
      fn title(&self) -> &str;
      fn preferred_height(&self, lane_w: i32) -> i32;
      fn draw(&mut self, s: &mut Surface, fonts: &mut Fonts, rect: Rect, focused: bool, now_ms: u64);
      fn on_char(&mut self, _c: char) {}
      fn on_key(&mut self, _code: u16) {}
      fn wants_close(&self) -> bool { false } // timer DONE-dismiss etc.
  }
  #[derive(Clone, Copy)] pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }
  ```
- Produces `deck::Deck`: `pub fn new(width: usize, height: usize) -> Deck`, `pub fn handle(&mut self, events: &[drivers::input::Event])`, `pub fn compose(&mut self, s: &mut Surface, fonts: &mut Fonts)`. `handle` owns pointer/shift/ctrl tracking (copy the pointer-scaling code from `ui/desktop.rs::handle`; add `ctrl` tracked from keycode 29).
- `main.rs` loop becomes: poll input → `deck.handle(&events)` → `deck.compose(&mut surface, &mut fonts)` → present (same pacing). Keep `ui/desktop.rs` compiling untouched for now (deleted in Task 8); silence its dead-code warnings with nothing — warnings are fine.

**Steps:**

- [ ] **Step 1:** Write `tokens.rs` (consts above), `card.rs` (trait + Rect above), and `deck/mod.rs` with `Deck { width: i32, height: i32, cards: Vec<Box<dyn Card>>, focus: usize, pointer: (i32,i32), shift: bool, ctrl: bool, left_down: bool }`. `compose`: fill field color; dot grid = single pixels at 24px pitch (`s.pixels[i] = GRID` directly, skip blending); 24px status strip: `fill_rect(0,0,w,24,PANEL)` + 1px hairline under it + small-caps text left `TINYOS · DECK` (ink-dim, 12px, drawn via a `pub fn label(s,&mut Fonts,text,x,y,color)` helper in deck/mod.rs that uppercases and letter-spaces) + right side `{uptime} · {heap MiB} · {clock}` using `mem::stats()` and `timer::uptime_ms`; cursor drawn last (`ui::cursor::draw`).
- [ ] **Step 2:** Swap the `main.rs` loop as per Interfaces.
- [ ] **Step 3:** `make build` → no errors.
- [ ] **Step 4:** QMP boot, screendump ~12s, Read png. Expected: graphite field with visible dot grid, status strip with labels, cursor. No cards.
- [ ] **Step 5:** `git add -A && git commit -m "deck: tokens, Card trait, field + status strip scaffold"`

### Task 2: Lane packer + focus + terminal card via hardcoded open

**Files:**
- Create: `kernel/src/ui/deck/layout.rs`, `kernel/src/apps/mod.rs`, `kernel/src/apps/terminal.rs`
- Modify: `kernel/src/ui/deck/mod.rs`, `kernel/src/main.rs` (add `mod apps;`), `kernel/src/term/mod.rs` (make `Terminal::handle` take plain `char`/`u16` — see below)

**Interfaces:**
- Produces `layout::pack(cards: &[Box<dyn Card>], width: i32, height: i32) -> Vec<Rect>`: lanes = `(width/470).clamp(1,3)`, lane width = `width/lanes - 16` margins; place each card into the currently shortest lane, top-down, 12px gaps, y starts under strip (24+12). Height = `preferred_height(lane_w)` clamped to remaining space (min 80).
- Produces `apps::terminal::TerminalCard` wrapping `term::Terminal` implementing `Card` (title "TERMINAL", preferred_height 420, draw = restyled `Terminal::draw` against `rect`: panel bg via `fill_rounded_rect(.., 4, PANEL)`, header 26px PANEL_HI with small-caps title + kill-dot placeholder, body text from rect origin).
- Modify `term/mod.rs`: replace `pub fn handle(&mut self, ev: &ShellEvent)` with `pub fn on_char(&mut self, c: char)` + `pub fn on_key(&mut self, code: u16)` (same match arms; `ShellEvent` import dropped). Remove `draw`'s dependency on `desktop::Window` — new signature `pub fn draw(&self, s, fonts, x: i32, y: i32, cols: usize, rows: usize, now_ms: u64)` computing cols/rows from the card rect in the adapter (`cols = (w-24)/CELL_W`, `rows = (h-26-16)/CELL_H`, with `CELL_W=9`, `CELL_H=19` moved into `term/mod.rs` from desktop).
- Deck: `open_card(Box<dyn Card>)` pushes + focuses last; focused card gets 2px SIGNAL edge (draw `fill_rounded_rect` border by drawing rect then inner rect, or 4 thin rects); typing routes `Event::Key` → focused card `on_char`/`on_key` (reuse `keycode_to_char`); click focuses the card under the pointer; `Ctrl+arrows`: pick nearest card center in that direction, else wrap.
- Temporary: `Deck::new` opens a `TerminalCard` so there's something to see (bar arrives in Task 3).

**Steps:**

- [ ] **Step 1:** Implement per Interfaces.
- [ ] **Step 2:** `make build` → no errors.
- [ ] **Step 3:** QMP: boot, type `sysinfo\n` (qcode per char, existing helper pattern), screendump. Expected: terminal card top-left lane, orange focus edge, sysinfo output inside card.
- [ ] **Step 4:** `git add -A && git commit -m "deck: lane packer, focus, terminal as a card"`

### Task 3: Command bar (open/close/help/unknown/open-cards/close)

**Files:**
- Create: `kernel/src/ui/deck/bar.rs`
- Modify: `kernel/src/ui/deck/mod.rs`

**Interfaces:**
- `bar::Bar { pub open: bool, input: String, cursor: usize, hint: Option<String> }`. `Bar::draw(s, fonts, screen_w, now_ms)`: 560px wide, top-centered 96px below strip, panel + hairline + 4px radius; SIGNAL caret block blinking 530ms; hint line under input in ink-dim. `Bar::on_char/on_key` edit like the terminal input (reuse nothing — 20 lines).
- `bar::Action` enum: `None, Dismiss, Open(&'static str), CloseFocused, Help, Calc(String), Timer(u64), Unknown(String)`; `Bar::submit(&mut self) -> Action` parses: known names `terminal|notes|monitor|clock` → `Open`; `close` → `CloseFocused`; `help` → `Help`; starts with `=` → `Calc(rest)`; `timer Ns|Nm` → `Timer(secs)`; else `Unknown(input)` (sets hint "no such command — try help", keeps bar open).
- Deck routing: when `bar.open`, ALL chars/keys go to the bar (Enter → submit → act; Esc → dismiss). `Ctrl+K` toggles. Click on empty field (no card hit) opens bar. `Open("terminal")` focuses existing terminal if present else spawns; same single-instance rule for monitor/clock; `notes` always spawns (Task 5 registers it — until then `Open("notes")` sets hint "not yet aboard"). `Help` opens a `HelpCard` (static text card listing commands — implement inline in `bar.rs` as `pub struct HelpCard;` with hardcoded lines, preferred_height 240).
- Remove the Task-2 temporary auto-open of TerminalCard (boot to empty field; bar is the way in).

**Steps:**

- [ ] **Step 1:** Implement per Interfaces.
- [ ] **Step 2:** `make build` → no errors.
- [ ] **Step 3:** QMP: `Ctrl+K` (qcode "ctrl" down, "k", ctrl up) → screendump (bar visible, caret orange); type `terminal\n` → screendump (card materialized, bar gone); `Ctrl+K`, `xyzzy\n` → screendump (hint line); `Ctrl+K`, `help\n` → screendump (help card).
- [ ] **Step 4:** `git add -A && git commit -m "deck: command bar with open/close/help and hints"`

### Task 4: Calculator in the bar

**Files:**
- Create: `kernel/src/ui/deck/calc.rs`
- Modify: `kernel/src/ui/deck/bar.rs` (Action::Calc handling stays in bar: submit computes and sets `hint = Some("= 42")`, keeps bar open with input intact)

**Interfaces:**
- `calc::eval(expr: &str) -> Option<f64>`: recursive-descent over `+ - * / ( )` and decimal literals; whitespace-tolerant; None on any parse error or division by zero. ~60 lines:
  ```rust
  struct P<'a> { b: &'a [u8], i: usize }
  impl P<'_> { fn expr(&mut self)->Option<f64>{...} fn term(...) fn factor(...) fn num(...) }
  pub fn eval(e:&str)->Option<f64>{ let mut p=P{b:e.as_bytes(),i:0}; let v=p.expr()?; p.skip_ws(); (p.i==p.b.len()).then_some(v) }
  ```
- Bar formats integers without decimals (`if v.fract()==0.0 && v.abs()<1e15 { format!("= {}", v as i64) }`), errors as hint "can't evaluate".

**Steps:**

- [ ] **Step 1:** Implement; `make build` → no errors.
- [ ] **Step 2:** QMP: `Ctrl+K`, `= 6*7\n` → screendump shows `= 42` under the bar; `= (2+3)*4.5\n` → `= 22.5`; `= 1/0\n` → "can't evaluate".
- [ ] **Step 3:** `git add -A && git commit -m "deck: inline calculator in the command bar"`

### Task 5: Notes and Clock/Timer cards

**Files:**
- Create: `kernel/src/apps/notes.rs`, `kernel/src/apps/clock.rs`
- Modify: `kernel/src/ui/deck/mod.rs` (wire `Open("notes")`, `Open("clock")`, `Timer(secs)`)

**Interfaces:**
- `NotesCard { lines: Vec<String>, cursor_line/col }` — chars insert, Enter splits, Backspace joins/deletes, arrows move. Title `NOTES`; preferred_height 260; body mono 15px; SIGNAL caret bar when focused. Multiple instances allowed.
- `ClockCard { timer: Option<{ end_ms: u64, total_ms: u64 }> }` — no timer: big clock (menu-bar 9:41 formula, Inter SemiBold ~44px) + small-caps date line. Timer: remaining `M:SS` in SIGNAL + thin progress rule draining; at zero renders `DONE` flashing (750ms cadence) and `wants_close` stays false (user closes). Title `CLOCK` / `TIMER`; preferred_height 150.
- Single-instance rule applies to clock (a `timer` command retargets the existing clock card into timer mode if one is open, else spawns).

**Steps:**

- [ ] **Step 1:** Implement; `make build` → no errors.
- [ ] **Step 2:** QMP: open `notes` twice + `clock`; type into the focused note; `Ctrl+arrows` to the other note, type there; screendump: 3+ cards across lanes, both notes hold distinct text, focus edge on the right card. Then `Ctrl+K`, `timer 5s\n`, wait 6s, screendump: DONE flashing.
- [ ] **Step 3:** `git add -A && git commit -m "deck: notes and clock/timer cards"`

### Task 6: Monitor card with live gauges

**Files:**
- Create: `kernel/src/apps/monitor.rs`
- Modify: `kernel/src/main.rs` (feed frame + event samples), `kernel/src/ui/deck/mod.rs` (wire `Open("monitor")`, expose `pub fn stats_tick(&mut self, frame_ms: u32, events: u32)` forwarding to an open monitor)

**Interfaces:**
- `MonitorCard { fps: RingBuf<u32; 120>, evs: RingBuf<u32; 120>, .. }` where `RingBuf` is a fixed array + write index (define in `monitor.rs`, 15 lines, no crate).
- `main.rs` measures per-frame: `frame_ms` from timer delta, `events.len() as u32` → `deck.stats_tick(..)` each loop.
- Draw: heap bar-meter (label `HEAP`, hairline track, INK fill, value in mono right-aligned, from `mem::stats()`); `FPS` sparkline from ring (1px columns, INK, current value big in mono, needle tick in SIGNAL); `INPUT` events/sec sparkline; `UPTIME` mono. Title `MONITOR`, preferred_height 300, single-instance.

**Steps:**

- [ ] **Step 1:** Implement; `make build` → no errors.
- [ ] **Step 2:** QMP: open `monitor`, wiggle pointer via abs events ~2s, screendump: sparklines non-flat, heap meter partial, FPS ≈ 60.
- [ ] **Step 3:** `git add -A && git commit -m "deck: monitor card with live gauges"`

### Task 7: Motion — materialize and re-pack animation

**Files:**
- Modify: `kernel/src/ui/deck/mod.rs`, `kernel/src/ui/deck/layout.rs`

**Interfaces:**
- Deck keeps `current: Vec<Rectf>` (f32 rects) alongside target rects from `pack`; each frame `current += (target-current) * min(1, dt/110ms)` per component (linear approach, clamp snap under 1px). New cards start at target rect scaled 0.98 around center with alpha ramp 0→1 over 100ms (alpha = draw card to its rect but pre-multiply the focus/panel colors' alpha — acceptable approximation: skip true alpha, animate only scale+position; card small-pop is enough).
- Card close: removed immediately, others slide via the same approach. No bounce.

**Steps:**

- [ ] **Step 1:** Implement; `make build` → no errors.
- [ ] **Step 2:** QMP: open `notes`, screendump at +30ms and +200ms after submit (two dumps): first shows mid-materialize (slightly smaller card), second settled. Close a card, dump mid-slide.
- [ ] **Step 3:** `git add -A && git commit -m "deck: mechanical materialize and re-pack motion"`

### Task 8: Splash re-skin, old shell removal, docs, x86_64 smoke

**Files:**
- Modify: `kernel/src/ui/splash.rs` (field bg `FIELD`, wordmark = small-caps letter-spaced `T I N Y O S` via deck label helper at ~40px, hairline progress rule 1px with SIGNAL fill instead of rounded bar)
- Delete: `kernel/src/ui/desktop.rs`, `kernel/src/ui/wallpaper.rs`; remove their `pub mod` lines from `ui/mod.rs`; move any surviving consts (CELL_W/CELL_H already moved in Task 2)
- Modify: `README.md` (Deck description + new screenshots), `docs/screenshots/` (replace desktop.png/terminal.png with deck shots, keep splash.png updated)

**Steps:**

- [ ] **Step 1:** Splash re-skin; delete old shell files; fix imports. `make build` → no errors, and `make build ARCH=x86_64` → no errors.
- [ ] **Step 2:** Full aarch64 QMP pass: splash dump, `Ctrl+K terminal`, open all five card types, final screendump → save as `docs/screenshots/deck.png` + splash to `docs/screenshots/splash.png`.
- [ ] **Step 3:** x86_64 smoke: boot headless (`-vga none`, TCG, 40s), `Ctrl+K`, `monitor\n`, screendump: Deck up on x86.
- [ ] **Step 4:** README rewrite of the GUI paragraphs (command-first cards, instrument identity, Ctrl+K).
- [ ] **Step 5:** `git add -A && git commit -m "deck: splash re-skin, remove legacy shell, docs + x86_64 smoke"`

## Self-Review Notes

- Spec coverage: bar behaviors (open/inline-calc/timer/close/help/unknown) → Tasks 3–5; lanes/focus/strip → Tasks 1–2; tokens/motion → Tasks 1, 7; five card types → Tasks 2, 5, 6; splash + deletion + both arches → Task 8. Click-to-focus + click-field-opens-bar → Tasks 2–3.
- Types: `Card`/`Rect` defined Task 1 and used consistently; `Action` defined Task 3, extended nowhere else; term signature change is contained in Task 2.
- No placeholders; QMP scripts follow the repo's established python/socket pattern.
