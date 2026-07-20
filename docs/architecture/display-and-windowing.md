# Display & windowing architecture

Status: design principle / direction (2026-07-20). Captures where the window
system belongs and how the current seams lead there. Not a task spec.

## The layering (three distinct jobs — keep them separate)

1. **Display server / window system** — owns the framebuffer, manages
   *multiple* windows (z-order, focus, chrome, dock, command palette), routes
   input to the focused window, and composites every client's surface to the
   screen. Today: `kernel/src/ui/shell/` (the `Shell` compositor + the
   `window` protocol / `ExternApp` host).

2. **The terminal** — takes *one* window and multiplexes text/cell content into
   it: line-world scrollback plus hosting a child's full-screen cell surface
   (vi/top) over the **console** protocol. This is a *console multiplexer*, the
   way tmux muxes panes without being an X server.

3. **Applications** — each opens a window (GUI apps: pixels, solitaire, edit) or
   runs as a console client hosted by the terminal (vi, top, sh).

**Principle:** the terminal is a *client* of the window system, never the window
system itself. The terminal hosting vi/top looks like windowing but isn't — it
puts different content into its single window. Folding windowing into the
terminal would be the wrong layering: the terminal would become a window
manager, and every non-terminal GUI app would have no host. So:

- Window management → the display server (a service).
- Content multiplexing inside one window → the terminal (a client).
- Never merge the two.

## Where the window system belongs: userspace (eventually)

The compositor is in the **kernel** today. Given the OS's microkernel direction
— shell → userspace, terminal → userspace, FS/PROC → broker services — the
compositor is the last large *policy* component still in the kernel, and by the
same reasoning it belongs in **userspace** as its own **display-server
process**. The concrete wins are the ones the earlier moves already bought:

- **Crash isolation:** a compositor bug becomes a recoverable crash, not a
  kernel panic.
- **Iteration without kernel risk:** chrome, layout, dock, palette, theming —
  exactly the churny code that shouldn't live in the kernel.
- **Consistency:** it's the same one-way trip the shell and terminal took.

**What the kernel keeps** (mechanism only): framebuffer ownership for the boot
splash and the panic screen; the input drivers (virtio-keyboard/tablet); and
the capability to hand the framebuffer (as a MemObj) plus an input-event
channel to the userspace compositor at boot. The compositor then maps the
framebuffer, receives input over a channel, and blits client surfaces it
already receives as shared MemObjs — no new copy, just one more capability
handoff.

## How the current seams lead there

This direction *validates* the existing roadmap rather than changing it:

- **SP1c — window broker = "connect to the display server."** Getting a window
  becomes: connect to the display server, mint a per-client window channel —
  the same broker pattern as SP0's FS/PROC brokers. Because it is
  protocol-over-channels, the broker is identical whether the server is
  in-kernel (now) or a userspace process (later), exactly as the FS broker is
  the seam for the eventual userspace `fsd`. **Frame SP1c's window broker
  explicitly as the display-server connection seam**, not as a kernel-internal
  shim, so no rework is needed when the server moves.
- **SP2 — delete the in-kernel `App` trait.** This shrinks the compositor to "a
  window server hosting only userspace clients" — precisely the clean,
  self-contained unit you would lift into userspace.
- **Later (SP3-ish) — compositor to userspace.** Spawn the compositor as a
  process; grant it framebuffer + input caps. Same shape as the `fsd` re-host.

## Non-goals / anti-patterns

- Do **not** put window management in the terminal.
- Do **not** keep the compositor in the kernel as a permanent home; treat its
  current location as a way-station.
- The kernel retains a *minimal* framebuffer path regardless (splash + panic),
  independent of the userspace compositor.

## Summary

Terminal = client (done right in SP1a/SP1b). Window system = its own service,
in the kernel today but on the same trip to userspace the shell and terminal
already made. The window broker (SP1c) is the seam; the `App`-trait deletion
(SP2) is the shrink; the userspace lift is the endgame.
