# tinyOS shell v2 — modern windowed desktop

## Goal

Keep the windows/desktop paradigm; replace the macOS-homage identity with a
modern "soft-dark neutral" design and modernize the experience: multiple
windows, snapping, resizing, and a command palette.

## Visual identity — tokens

| Token | Value | Use |
|---|---|---|
| field | `#0F1222` | wallpaper base (deep navy) |
| blob-a | `#7C5CFF` | wallpaper radial blob (violet) |
| blob-b | `#2BB8D9` | wallpaper radial blob (cyan) |
| surface | `#1C1E26` | window body (single unified panel) |
| surface-hi | `#252834` | hover states, palette field |
| border | white @ 8% | 1px window/panel outline |
| accent | `#7C5CFF` | focused-window glow, caret, running dots, snap preview |
| text | `#E6E8F0` | primary |
| text-dim | `#8A8FA3` | secondary, status bar |

- Radii: 14px windows/panels, 10px app icon tiles, pill = h/2.
- Shadows: large soft (reuse layered-rounded-rect falloff, wider spread than
  v1). No gradients on surfaces; the only gradient lives in the wallpaper.
- Window chrome: no titlebar band — inline title row (icon glyph + title,
  left) and one ghost `×` circle (right). Focused window: 1.5px accent
  border; unfocused: white-8% border.
- Wallpaper: pre-rendered mesh gradient — field base + 2-3 large soft
  radial blobs (violet low-left, cyan upper-right), subtle. Existing
  box-blur backs the frosted dock/status bar as today.
- Splash: keep layout; field background, accent progress fill.
- Fonts unchanged (Inter + JetBrains Mono).

## Experience

### Window manager
- Multiple windows: `Vec<Window>`, draw order = z-order, click-to-front,
  one focused window (accent border) receives keys.
- Window hosts an `App` trait object: `title()`, `glyph()` (one-char icon),
  `preferred_size(screen)`, `min_size()`, `draw(surface, fonts, body,
  focused, now_ms)`, `on_char`, `on_key`.
- Title-row drag moves. Bottom-right 18px corner grip resizes, clamped to
  `min_size` and screen. Ghost `×` closes.
- Snapping: dragging with pointer within 16px of left/right edge shows an
  accent-outline half-screen preview; release snaps. Top edge = maximize.
  Dragging a snapped/maximized window away restores its remembered size.
  `Ctrl+Left/Right` = snap halves, `Ctrl+Up` = maximize, `Ctrl+Down` =
  restore. Snapped geometry excludes status bar and dock zones.

### Apps (recovered from discarded Deck branch, reflog `fb45af2`)
- Terminal (existing shell), Notes (multi-instance), Monitor (heap/fps/
  input gauges; fed per-frame), Clock/Timer (`timer 5m` via palette).
- App logic is adapted to the `App` trait; visual restyle to v2 tokens
  (no small-caps labels — normal-case Inter, dim secondary text).

### Launchers
- Dock: frosted bottom-center pill, 4 icon tiles (mono glyphs), accent
  running dot under open apps; click opens or focuses (notes: opens a new
  instance if none open, focuses most recent otherwise; a second click
  while focused opens another instance).
- Status bar: slim frosted top bar — "tinyOS" wordmark left; uptime, heap,
  clock right, in text-dim.
- Command palette: `Ctrl+K` overlay (centered, surface-hi, accent caret):
  app names open/focus, `= expr` inline calculator (recovered parser),
  `timer Ns/Nm`, `close`, `help` hint line. Esc dismisses.

## Architecture

- `ui/shell/` new module: `tokens.rs`, `wm.rs` (Window, drag/resize/snap
  state machine), `app.rs` (App trait), `dock.rs`, `statusbar.rs`,
  `palette.rs`, `wallpaper.rs` (mesh gradient), `mod.rs` (Shell owning all
  of it; input routing incl. Ctrl tracking).
- `apps/` hosts App impls: `terminal.rs`, `notes.rs`, `monitor.rs`,
  `clock.rs`. `ui/desktop.rs` + old `ui/wallpaper.rs` deleted at the end.
- Kernel/drivers/gfx untouched; both arches inherit the shell.

## Milestones

- **W1**: tokens, mesh wallpaper, restyled chrome/status bar/dock, single
  terminal window in the new skin.
- **W2**: multi-window (z-order, click-front, drag, close), apps recovered
  as windows, dock wired to all four.
- **W3**: resize grip + edge snapping + keyboard snapping.
- **W4**: command palette + splash re-tint + legacy deletion + README/
  screenshots + x86_64 smoke.

## Verification

Per milestone: `make build` clean; QMP-scripted boot on aarch64 driving
pointer/keys; screenshot assertions (window moved/snapped/resized, palette
open, apps rendering). W4 adds the x86_64 TCG smoke boot. Work happens on
branch `shell-v2`; final acceptance is `make run` in person before merge.
