# tinyOS filesystem tree + capability substrate — overview

Status: design. Supersedes the ad-hoc `/apps` + `/data/<name>` + `/shared` layout.
Companion specs (this date): secrets-service, powerbox-v2, service-supervisor,
package-generations, multi-user-admin. Builds on: `2026-07-18-tinyfs-design.md`,
`2026-07-18-app-api-design.md`, `2026-07-19-service-brokers-design.md`.

## Why

The committed tree was essentially `/apps` (flat ELF binaries) plus lazily-created
`/data/<name>` and `/shared`. This spec defines the default tree from scratch,
fixing what legacy layouts bake in (FHS's `/usr` split, `/etc` grab-bag,
config/cache/state sludge in `/var`, apps sprayed across the tree, code and user data
mixed together). Adversarial pressure-testing against ~100 real-world file categories
showed the *tree* is sound but that a capability OS needs several subsystems legacy
buries in `/etc`, `/var`, uid bits, and daemons: a secret store, a richer powerbox, a
service supervisor, a package manager with rollback, and a multi-user/admin model.
Those are specified in the companion documents; this document is the frame.

## Foundational decisions

- **Provenance-first tree.** One top-level axis: *who provided it* — the OS
  (`/system`), this machine (`/local`), or a specific user (`/users/<u>`). Each root is
  self-similar (bin/apps/share/…); read-only vs writable is a per-subdirectory
  property, never a top-level split (that was the `/usr/local`-vs-`/opt` mistake).
- **Capability IS the principal.** No ambient root namespace; no uid/gid; no permission
  bits; no MAC. A process reaches only what it was handed as an unforgeable channel
  handle. **Login mints the capability to `/users/<user>`** — you cannot *name* another
  user's home without a granted handle, so cross-user privacy is the *absence of a
  handle*, requiring zero enforcement code.
- **No root account.** "Admin" is not an identity but a bundle of fine-grained
  capabilities a user is *authorized to acquire*, exercised **just-in-time**, per
  action, with consent, scope- and time-boxed, and audited. Least *standing* privilege:
  even an admin's session is cold by default.

## The tree

```
/system/            OS image — read-only. Pinned classes (kernel-ref, CA-trust,
                    auth) are never name-shadowed by lower layers.
  bin/ apps/ share/ defaults/
  services/         authd sessiond secretd userd homesd logd powerboxd svcd
/local/             machine, non-OS
  bin/ apps/ share/            installed code (resolved user→local→system)
  services/                    pkgd netd devd + *.manifest supervision sidecars
  config/                      network devices hostname timezone
  state/<svc>/                 durable userless service data (docroots, DB dirs, spools)
  cache/                       machine-wide evictable (shader/font/icon caches)
  log/                         system + per-service logs + audit sink
  secrets/                     machine vault (secretd only)
  registry/                    package DB, generations, accounts, service enable-state
  shared/                      cross-app machine-wide data
/users/<u>/         reached ONLY via the login-minted capability
  Documents/ Downloads/ Pictures/ Music/ Video/ Templates/
  bin/ apps/ share/            user-installed code
  apps.data/<name>/{config,data,cache,state}   ← a user app's jail root ("/")
  registry/                    powerbox grants, default-app associations, autostart
  secrets/                     per-user vault (unlocked at login)
  trash/
/volumes/<label>/   foreign filesystems graft here, capability-gated (removable,
  boot/             network, AND the ESP: A/B kernel slots, updater-only)
/tmp/  +  /tmp/<svc>/   ephemeral per-boot; per-service runtime scratch
```

Rules:
- **Resolution is user → local → system** (a user install shadows a machine install
  shadows the OS); same order for `bin/`, `apps/`, `share/`. Implemented as a resolver
  service, not a union mount (see package-generations).
- **App code** may live in any provenance tree; a user app's **data** is always in the
  running user's `/users/<u>/apps.data/<name>`, whose four subdirs (`config data cache
  state`) are what the jailed app sees as its own `/`.
- **Config cascade:** bundle `defaults/` → `/local/config/<name>` (machine) →
  `/users/<u>/apps.data/<name>/config` (user).
- **Deliberately absent:** no `/dev` (devices are capabilities), no `/proc`, `/mnt`,
  `/etc`, `/var`, no `/usr` split, no `/opt`.

## The unification

The whole substrate rests on **five existing primitives**:
1. Broker `OP_CONNECT → mint() → moved-handle` (`kernel/src/fs/server.rs`, `svc.rs`).
2. `OP_OPEN_DIR` narrowing — can only narrow (`kernel/src/fs/service.rs`).
3. Hierarchical revocation — drop parent → children die.
4. Default-deny manifest tokens (`kernel/src/obj/loader.rs::manifest`).
5. `Dir::into_handle` delegation (`apps/sdk/src/fs.rs`).

Only **five new primitives** were required, one per problem domain:
- **Oracle** — a handle that *uses* a secret you can never *read* (secrets/auth).
- **Nexus** — a named broker where readiness == published (supervisor).
- **`rights` bitmask + `OP_WATCH`** — attenuated FS modes + change events (powerbox).
- **Content-addressed store + generations** — atomic rollback (package).
- **Authority-box + leased attenuated child** — JIT consented elevation (multi-user).

## Service roster

- Kernel-spawned root: `svcd` (supervisor).
- TCB (`/system/services/`): `sessiond`, `authd`, `secretd`, `userd`, `homesd`, `logd`,
  `powerboxd`, `nexus` (in-kernel first).
- Installed admin services (`/local/services/`): `pkgd`, `netd`, `devd`.

## Bootstrap tag map (reconciled)

Existing 1–6: `CONSOLE SHELL FS PROC FS_BROKER PROC_BROKER`. New: 7 `NEXUS`,
8 `SECRETS`, 9 `SECRETS_BROKER`, 10 `VAULT`, 11 `SESSION`, 12 `POWERBOX`,
13 `OPEN_FILE`, 14 `FS_SCRATCH`, 15 `FS_CONFIG`, 16 `PKG`, 17 `MACHINEKEY`,
18–25 `ADMIN_{USERS,PKG,SVC,NET,DEV,SECRETS,LOGS,HOMES}`, plus private-range
mint-broker tags between init and sessiond.

## Build roadmap

1. **Tree + jail repoint (done).** Seed the tree at image creation; repoint the
   launcher jail from `/data/<name>` to `/users/user/apps.data/<name>` (single implicit
   user until login); terminal cwd → `/users/user`. `/apps` stays flat ELF.
2. **Supervisor + Nexus**, then **Secrets**, **Powerbox v2**, **Package + generations**,
   **Multi-user + admin** — each its own milestone; see companion specs. Login (phase
   multi-user) replaces the single implicit `DEFAULT_USER`.
