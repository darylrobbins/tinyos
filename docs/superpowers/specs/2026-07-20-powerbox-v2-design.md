# Powerbox v2 — design

Status: design. Part of the capability substrate (see
`2026-07-20-fs-tree-and-substrate-design.md`). The mechanism by which a sandboxed app
(jailed to `/users/<u>/apps.data/<name>`) gains access to the user's real files and
other resources *outside* its jail — via persistent, recursive, directory-scoped
capabilities. A per-file picker is unusable for repos (`.git` = thousands of files,
`node_modules` = 100k+), bulk downloads, screenshots, sidecar files, and sync-conflict
files.

## Core idea: a grant is an `OP_OPEN_DIR` mint from a different root

`FsService::op_open_dir` already mints a fresh FS channel jailed to a subdir, pushed to
`children` (pumped-with-parent, dropped-on-close = hierarchical revocation), returned
as a moved handle — the app receives a *new capability*, never a widening of its jail.
Powerbox v2 is exactly that op, but the parent connection is **`powerboxd`'s
login-granted `/users/<u>` root**, and the picked path lives outside the requesting
app's jail. `powerboxd` (`/system/services/`, pinned) hands the app the resulting
channel via `Dir::into_handle`; the app's own `TAG_FS` is untouched. A grant is just a
`Dir` rooted at a different subtree, so the SDK reuses the existing `Dir` type verbatim.

## Grant minting

New tag `TAG_POWERBOX`, granted to apps declaring the `powerbox` manifest token. New
protocol `crates/abi/src/powerbox.rs`: `OP_PICK_FOLDER{modes}` / `OP_PICK_FILE{modes}`
(draw the trusted picker — only `powerboxd` can enumerate `/users/<u>` — mint jailed
channel via `PowerboxServer::mint_jailed(jail, base, rights)`, a generalization of
`FsServer::mint`), `OP_REOPEN{grant_id}` (re-mint a remembered grant, no UI),
`OP_FORGET`, `OP_LIST`, `OP_OPEN_WITH`. A dedicated `PowerboxServer` pool (not
`OP_OPEN_DIR` children of the root, whose `MAX_SUBDIRS=16` is too few) pumps grants the
same way `FsServer` does.

## Grant modes (`rights` bitmask)

Adds `rights: u32` to `FsService` (today it enforces only per-fd write + the jail):
`PB_READ`, `PB_WRITE`, `PB_CREATE`, `PB_CREATE_SIBLING`, `PB_RECURSIVE`, `PB_WATCH`,
`PB_REMEMBER`. Rights are narrow-only: `OP_OPEN_DIR` gains a leading `rights:u32`, a
child's rights = `parent.rights & requested` (absent word = inherit ALL, backward-
compatible). So the jail's "can only narrow" invariant now also holds for modes.

## `OP_WATCH` (new — build now)

`OP_WATCH{path}` → `R_WATCH` + a dedicated event channel handle (same handle-in-reply
pattern as `OP_OPEN_DIR`); mutating ops (`write`/`mkdir`/`remove`/`rename`) broadcast
coarse dir-level events to watchers whose jail is a path-prefix. All writes funnel
through `FsService`, so the notify plumbing is tractable; scope it best-effort. Needed
by file managers, dev tools, and sync engines — polling doesn't scale.

## Persistence

Remembered grants live in `/users/<u>/registry/powerbox/grants` (powerboxd-owned,
append-only log + tombstones). Record: `grant_id, app_id, path (under /users/<u>),
modes, kind, created_gen, last_used_gen, live`. `app_id` = the app's stable install
name (not a binary hash), so updates keep grants. Re-hydration: the app persists its
`grant_id`s in its own `apps.data/<name>/config` and calls `OP_REOPEN` at startup;
`powerboxd` validates ownership and re-mints silently (the security-scoped-bookmark
equivalent).

## Revocation & audit

`powerboxd` is the single registry of standing grants. `powerbox.admin` (held only by
the shell/Settings) enables `OP_LIST_ALL`/`OP_REVOKE` for a user-facing "App
Permissions" view. Revoke drops the grant's `FsService` → the app's `Dir` reads closed
(hierarchical revocation); the record is tombstoned. Logout drops `powerboxd`'s root cap
→ every grant dies at once.

## Cross-jail transfers (capability moves, never path passing)

- **Drag A → B:** the shell owns both window channels; on drop it asks `powerboxd` to
  mint a one-shot file grant and moves that handle into B's `OP_DROP` message (new window
  ops `OP_OFFER`/`OP_DROP`). The path never crosses the boundary.
- **Launch-by-association:** double-click reads `/users/<u>/registry/associations`
  (`ext → app_id`), spawns the default app with a freshly-minted single-file grant as a
  bootstrap grant (`TAG_OPEN_FILE`), basename in argv.

## Decisions

- Enforce modes in-service via `rights` on `FsService` + narrow-only `OP_OPEN_DIR`.
- File grant = dir-jail to the parent + `sibling_stem` (read the picked file; write/
  create limited to `stem.*`) — enables `.xmp` sidecars and sync-conflict files.
- Build `OP_WATCH` now (event channel).
- Re-hydration via app-stored `grant_id` + `OP_REOPEN`.
- Bind grants to `path + app_id` (inode-follow across moves is a later item).

## Tree & surface

`/system/services/powerboxd`; `/users/<u>/registry/{powerbox/grants,associations}`;
grants target anywhere under `/users/<u>/` and cannot escape it. New tags 12
`POWERBOX`, 13 `OPEN_FILE`; manifest tokens `powerbox`, `powerbox.admin`; FS op
`OP_WATCH` + `rights` word on `OP_OPEN_DIR`; window ops `OP_OFFER`/`OP_DROP`; SDK
`apps/sdk/src/powerbox.rs`.
