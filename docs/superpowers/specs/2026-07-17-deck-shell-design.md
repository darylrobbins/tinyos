# Deck — tinyOS's command-first card shell

## Thesis

Replace the macOS-homage desktop with an original interaction model. The OS
is an instrument, not a desk: one command surface, a field of live cards,
zero ornamental chrome. Nothing overlaps, nothing hides, everything is
reachable from the keyboard; the mouse works everywhere but is never
required.

## Interaction model

### Command bar
- `Ctrl+K` summons it (top-center drop-in); `Esc` dismisses; clicking the
  empty field also summons it.
- It is launcher, menu, and control panel:
  - `terminal`, `notes`, `monitor`, `clock` — open cards (notes allows
    multiple instances; the others focus their existing card if open).
  - `= <expr>` — inline calculator; result renders in the bar itself
    (integer/float, `+ - * / ( )`), no card spawned.
  - `timer <N>m` / `timer <N>s` — spawns a countdown clock card.
  - `close` — closes the focused card. `help` — opens a help card listing
    the commands.
- Unknown input → quiet inline hint under the bar ("no such command"),
  never a dialog.

### Cards and lanes
- Every running thing is a card in a lane layout: up to 3 fixed-width lanes
  (screen width / ~470px, min 1), cards auto-placed left-to-right then
  top-down within lanes. No overlap, no manual move/resize in v1.
- Card anatomy: hairline border, header row (small-caps title + kill-dot to
  close), body. Each card type declares a preferred height.
- Exactly one focused card: signal-color edge, receives all typing.
  `Ctrl + arrow keys` move focus spatially; clicking a card focuses it.
- Closing a card re-packs the lanes with a ~120ms mechanical slide.
- Cards materialize with 0.98→1 scale + fade over ~100ms. No bounce
  anywhere.

### Status strip
- 24px top strip, always present, not interactive.
- Left: `TINYOS · DECK`. Right: uptime, heap used, clock. Small-caps
  micro-labels.

## Visual language — "instrument panel"

| Token | Value | Use |
|---|---|---|
| field | `#17181A` | background, with dot grid `#26282B`, 24px pitch |
| panel | `#232528` | card bodies, bar |
| panel-hi | `#2A2D30` | card headers, hover |
| hairline | `#3A3E42` | all borders/rules (1px) |
| ink | `#E8EAEC` | primary text |
| ink-dim | `#9BA1A6` | small-caps labels, secondary |
| signal | `#FF6A00` | live/active ONLY: focus edge, caret, gauge needles, running timer |

- Type: Inter (UI; small-caps labels = uppercase + wide tracking),
  JetBrains Mono (all data readouts, terminal). Big numerals on glanceable
  cards.
- Shapes: 4px radius, no blur, no gradients, no glass. Depth = hairline +
  one 1px-offset shadow line.
- If it's orange, it's alive. Nothing else is colored.
- Splash re-skinned to match: small-caps `T I N Y O S` wordmark, hairline
  progress rule, graphite field.

## v1 cards

1. **Terminal** — the existing shell (`term::Terminal`) rehomed as a card.
2. **Monitor** — live gauges: heap used (bar-meter), FPS (sparkline over a
   ring buffer), uptime, input events/sec. The identity showpiece.
3. **Notes** — minimal text editing (chars, backspace, enter), multiple
   instances, contents persist in RAM while the card is open.
4. **Clock/Timer** — large-type clock; `timer 5m` variant counts down in
   signal orange, flashes DONE.
5. **Calculator-in-bar** — see command bar.

## Architecture

- `ui/deck/` replaces the old shell. `ui/desktop.rs`, dock, menu bar, and
  wallpaper are deleted (replacement, not a mode). Cursor and splash stay.
- `Card` trait: `title()`, `preferred_height(lane_width)`,
  `draw(surface, fonts, rect, focused, now_ms)`, `on_char(c)`,
  `on_key(code)`. Apps live in `kernel/src/apps/` as implementations:
  `terminal.rs`, `monitor.rs`, `notes.rs`, `clock.rs`.
- Deck owns: card list (`Vec<Box<dyn Card>>`), lane packer, focus index,
  command bar state + calc parser (`ui/deck/calc.rs`), status strip,
  animation clock.
- Input layer gains Ctrl tracking (keycode 29) beside existing Shift.
- Kernel, drivers, gfx, fonts untouched; both architectures inherit Deck.

## Milestones

- **D1 (model works end-to-end):** field + strip + bar + lane packing +
  focus + terminal card. `Ctrl+K → terminal → type in it → close`.
- **D2 (full v1):** monitor, notes, clock/timer, calc-in-bar, materialize/
  re-pack motion, splash re-skin.

## Verification

QMP harness (existing pattern): boot headless → `Ctrl+K`, `terminal` →
screenshot; open `notes` twice + `monitor` + `clock` → screenshot lanes and
focus edge; `Ctrl+arrows` cycle → screenshot; `= 6*7` → screenshot inline
result; `timer 5s` → wait → screenshot DONE state. Repeat the smoke boot on
x86_64. Final acceptance: `make run` in person.
