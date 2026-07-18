# Meridian — tinyOS shell reskin + surfaces from the Meridian OS design

Source of truth: `docs/reference/meridian-os.html` (downloaded from the
user's claude.ai design project "Modern OS UI design", file
`Meridian OS.dc.html`). Values below are extracted from it verbatim.
Builds on the shell-v2 window manager (branch `shell-v2`).

## Tokens (dark theme)

| Token | Value | Use |
|---|---|---|
| bg | `#07090d` | field base |
| wall1 | teal `rgba(95,212,196,.12)` | radial wash at 76% 12%, r~1100x750 |
| wall2 | violet `rgba(122,110,228,.10)` | radial wash at 12% 88%, r~1000x700 |
| win | `rgba(17,20,26,.82)` | window glass (blurred backdrop sample + tint) |
| glass | `rgba(14,17,23,.72)` | dock/launcher/pills glass |
| card | `rgba(255,255,255,.045)` | inputs, tiles, inset panels |
| card2 | `rgba(255,255,255,.08)` | active tabs, icon tiles, hover |
| stroke | `rgba(255,255,255,.09)` | all hairlines |
| stroke2 | `rgba(255,255,255,.16)` | kbd chips, emphasized borders |
| tx | `#e8ecf2` | primary text |
| tx2 | `#9aa4b5` | secondary |
| tx3 | `#5f6879` | tertiary/labels |
| acc | `#5fd4c4` | THE accent (teal) |
| accTx | `#052a24` | text on accent |

Secondary hues (icons/syntax only): amber `#e2b86b`, blue `#7fb2ff`,
violet `#b79bff`, red `#ff9e9e`.

- Radii: windows 14, dock/launcher/pills 18, icon tiles 13, inputs/cards
  8–12. Shadows: single huge soft (`0 24px 80px rgba(0,0,0,.5)` → our
  layered approximation, wide spread).
- Orb: 44px tile, `linear-gradient(135deg, acc, rgba(122,110,228,.9))`,
  glyph `✦` (fallback `*`), dark text `#08110f`, teal outer glow.
- Type: Geist (UI) + Geist Mono (labels/data/terminal), replacing
  Inter/JetBrains Mono. Micro-labels: mono 11px uppercase, wide tracking.
  Window titles 13px w600. Big headings tight.

## Layout & surfaces

- **No top bar.** Two floating glass pills at the bottom:
  - Dock (center, h64, r18): orb button (toggles launcher) · 1px separator
    · app tiles 44px r13 with per-app colored glyph (terminal `>_` teal,
    notes `N` amber, monitor `~` blue, clock `()` violet) · 4px teal
    running-dot under open apps. Click = open/focus (notes rule as today).
  - Clock pill (right, h64, r18): `HH:MM` mono 15 w600 over date 11px
    tx2. Click toggles quick settings.
- **Windows**: glass (frosted sample of pre-blurred wallpaper tinted with
  `win`), r14, 1px stroke; focused = 1px `stroke2` + subtle teal 1px?
  No — focused window per mock has no accent border; focus is shown by
  z-order + slightly stronger border (`stroke2`). Title row 44px with
  bottom hairline: app glyph (mono, acc/app hue) + title 13 w600, then
  right controls `– □ ✕` mono 13 tx3 (hover tx): minimize to dock,
  maximize/restore toggle, close. Resize grip + snapping unchanged from
  shell-v2 (snap preview recolored teal).
- **Launcher** (Ctrl+K, replaces palette visuals; same Action engine):
  bottom-centered 880px (clamped), r18 glass: header row (orb 34px + input
  17px "Ask tinyOS anything — run, open, calculate…" + `⌘K` chip) /
  `SUGGESTED` label + 3 action rows (✦ teal, real actions: open monitor,
  timer 5m, `= …` example) / `APPS` label + tile grid of the 4 apps
  (46px tiles, colored glyphs, name 11.5px tx2) / footer: `daryl ·
  tinyOS 0.1 "meridian"` + ghost buttons Lock (works) Restart Shut down
  (hints). Typed commands work exactly as before (names, `= expr`,
  `timer`, `close`, `help`).
- **Quick settings** (clock pill toggle, r16 glass, right-bottom): 3-col
  tile grid — Lock (works), Timer 5m (works), About (hint) — then a
  `SYSTEM` section: heap meter, FPS, uptime rows in mono; footer
  `tinyOS 0.1 "meridian"`.
- **Lock screen** (Ctrl+L, Lock tiles/buttons): full-screen field wash,
  giant mono clock (~120px canvas-scaled), date line, 74px gradient
  avatar circle with `D`, `daryl`, "press ↵ to unlock" (Enter unlocks;
  any typing ignored otherwise).
- **Minimize**: `–` hides the window (kept in Vec, `hidden` flag);
  dock dot stays; dock click or launcher open restores + focuses.
- Terminal restyle: prompt `daryl@tinyos ~ ❯` (user teal, path tx3,
  chevron teal; `>` fallback if `❯` missing from font), body on
  transparent window glass, block caret teal.
- Splash: field bg, `tinyOS` wordmark Geist 64, teal progress fill.
- Monitor/notes/clock restyle to tokens (needles/timer teal).

## Out of scope (v1)

Light theme, accent switching, wifi/bt/battery chrome, Settings app,
browser/chat/music apps, wallpaper thumbnails.

## Verification

QMP flows per milestone (screenshots): reskinned desktop w/ dock+clock
pills; launcher open with grid; minimize→dot→restore; quick settings;
lock/unlock; snap preview teal; x86_64 smoke. Final: `make run` in person.
