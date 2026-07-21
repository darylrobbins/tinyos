# Package manager + system generations — design

Status: design. Part of the capability substrate (see
`2026-07-20-fs-tree-and-substrate-design.md`).

## The constraint that shapes everything

tinyfs (`crates/tinyfs/src/fs.rs`, `layout.rs`) keeps only **two** checkpoints —
double-buffered blocks 1/2, mount picks highest `generation`, and superseded blocks
rejoin the allocator after `commit()`. So tinyfs gives exactly one primitive: **atomic
single-commit durability = rename-into-place** (a crash leaves the prior tree intact;
CRC rejects a torn slot). It does **not** retain history. Therefore **system
generations are built Nix-style at the file layer** — an immutable content-addressed
bundle store + tiny generation-descriptor files — using tinyfs's commit only as the
transaction/rename primitive.

## Package format + resolution

A package is a content-addressed immutable dir `store/<hash>/`: `app.bin` (ELF,
carrying its `.tinyos_abi` caps blob), `app.manifest` (name, semver, kind
`cmd|app|lib|service`, deps, provides, service-enable-default), `resources/`,
`defaults/` (pristine config), `SIGNATURE`. Because each package is self-contained, no
two share a file tree — **the per-file ownership DB that dpkg/rpm/pacman need
dissolves**.

**Shadowing is a resolver service, NOT a union mount** (a uniform union mount is the
escalation hole). A name-lookup service answers `resolve(kind, name)`: pinned classes
(kernel-ref/ca-trust/auth) resolve only from `/system`; else search user → local →
system, first hit wins, candidates drawn from the current generation's active set;
returns `{store_path, entry, version}`. The shell's `run` (`apps/shell/src/main.rs`,
currently the flat `fs::read("/system/apps/{name}")`) switches to
`resolve(cmd, name)`.

## Transactional package DB (`/local/registry`)

```
store/<hash>/        content-addressed immutable bundles (= reinstall/rollback cache)
db/packages.idx      name -> [(version, store-hash, tree, sig-key-id)]
db/conffiles.idx     (pkg,ver) -> [(rel-path, pristine-default-hash)]
db/services.idx      service -> {enabled, pkg, store-hash}
generations/<N>      immutable generation descriptors
generations/current  the ONE mutable pointer (names the active generation)
txlog                crash-safe idempotent intent log
incoming/<tmp>       download+verify staging
```

Single writer **`pkgd`**. Atomic install/update: (1) download to `incoming/`, verify
sig+hash; (2) `rename incoming/<tmp> → store/<hash>` (one tinyfs commit → present-or-
absent, never partial); (3) write immutable `generations/N+1`; (4) flip
`generations/current`; (5) settle txlog. Crash before (4) → still on gen N, orphan
harmless, GC'd. No fsck, no half-install.

**3-way conffile merge:** live config lives outside the bundle (`/local/config`,
`/users/<u>/apps.data/<n>/config`); bundles ship pristine `defaults/`. On upgrade,
compare old-default / new-default / live: unmodified → take new; user-changed → keep
live; both-changed → keep live + drop `*.new` + flag.

## System generations

`generations/<N>` = `{gen, parent, active[per-pkg: name version store-hash tree
service:on|off], conf[baseline hashes], kernel[slot A|B + kernel-hash], pins}`. A
generation is a snapshot of {installed set + active system/local pkgs + service enable
state + config baselines + kernel slot}. **Per-package convenience and atomic rollback
are the same mechanism**: every mutation writes one new generation; rollback =
`current = parent(N)`, one commit. GC keeps last K + current + pinned; the store *is*
the cache, so rollback never touches the network.

## Shadow-exempt pinned trust

kernel-ref, ca-trust, auth are never name-shadow-resolved from a lower layer. **Primary
defense: they aren't name-resolved at all — they're capabilities** (auth = the
authenticator oracle; ca-trust = a cap to the TLS/Secrets service; kernel-ref = the
updater's ESP cap). **Defense-in-depth:** the generation's `pins` list makes
`resolve()` short-circuit those classes to `/system`, and `pkgd` refuses to install a
package that `provides` a pinned class into local/users. Pins are OS config, never
app-declarable.

## Kernel / boot updates

ESP `/volumes/boot/` (updater-only): `slotA/kernel`, `slotB/kernel`, `boot.cfg{active,
try, confirmed}`. The kernel is another package but its activation is a **reboot**, not
a pointer flip. Update writes the inactive slot, verifies, sets the new generation's
`kernel` line, flips `boot.cfg.active` with `try=true`; the bootloader auto-reverts if
the OS doesn't set `confirmed` early (A/B failsafe). A rollback whose `kernel` slot
differs from `boot.cfg.active` requires a reboot; userspace-only rollbacks don't. One
descriptor binds kernel slot + userspace set so they roll back together.

## Decisions

- Nix-style content-addressed store + generation descriptors (forced by 2-checkpoint
  tinyfs).
- Shadowing via a resolver service, not a union mount (closes the pinning hole).
- Self-contained immutable bundles ⇒ no per-file ownership DB; shared libs are their own
  `lib` bundles (store dedups identical hashes).
- Pinned trust delivered as capabilities + a name-pin exception list (defense-in-depth).
- Single `pkgd` writer with a trivial txlog.

## Tree & surface

`/local/registry/{store,db,generations,current,txlog,incoming}`; `/system/{bin,apps,
share}` + `/local/{bin,apps,share}` + `/users/<u>/{bin,apps,share}` resolved by
`pkgd`; `/volumes/boot` A/B kernel slots. New service `pkgd` (jailed to
`/local/registry`), `crates/abi/src/pkg.rs` (read: `OP_RESOLVE/CURRENT_GEN/LIST/
GEN_LIST`; admin: `OP_INSTALL/UPDATE/ROLLBACK/SET_SERVICE/GC`); tag 16 `PKG`; manifest
token `pkg.admin`. This milestone also enables `<tree>/apps/<name>/` bundle
directories (replacing the flat `/system/apps/<name>` ELF).
