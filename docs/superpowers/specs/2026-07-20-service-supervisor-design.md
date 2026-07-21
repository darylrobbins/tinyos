# Service supervisor — design

Status: design. Part of the capability substrate (see
`2026-07-20-fs-tree-and-substrate-design.md`). The capability broker gives *discovery*
but not lifecycle, ordering, supervision, or userless-data placement. tinyOS bans pid
files and unix-socket rendezvous by design; this design does everything those files
did without them.

## Core idea: readiness == published-to-broker (the Nexus)

Add one generalized named broker, the **Nexus**, and ordering, readiness, and
socket-activation-race-avoidance collapse into a single primitive already present in
spirit (`OP_CONNECT`→mint→moved-handle + peer-close reaping):

- A service becomes **ready** the instant it `OP_PUBLISH`es its service-connection
  handle to the Nexus under a name (`"net"`, `"secrets"`, …).
- A dependent does `OP_LOOKUP("net")` and **blocks on the channel signal** until that
  publish happens — a kernel channel wait, no polling, no rendezvous file, no TOCTOU.
- When the provider dies, its published handle's peer closes; the Nexus **reaps the
  registration** exactly like `FsServer` reaps dead connections. Readiness auto-clears.

This replaces systemd's `After=` (the dependent's blocking lookup can't return until
the provider published), `Requires=` (lookup timeout ⇒ mark dependent failed; provider
death auto-reaps so a later lookup re-blocks — dependency is *live*), and
socket-activation (publish happens exactly at readiness; no early-bind window, and it
also handles provider *restart* with no client-visible artifact).

## Declaration: sidecar plain-text manifest

Supervision metadata lives in `/system/services/<name>.manifest` /
`/local/services/<name>.manifest` beside the binary — NOT the ELF caps stamp — so
`svcd` can plan start order and run enable/disable without loading every ELF (caps
still come from the ELF stamp; the loader is unchanged). Same one-token-per-line
grammar: `service`, `provides:<name>`, `requires:<dep>` (hard), `wants:<dep>` (soft),
`after:<dep>` (ordering only), `state` (durable rw jail at `/local/state/<name>`),
`scratch` (ephemeral rw jail at `/tmp/<name>`), `config` (read-only `/local/config`),
`restart:on-failure|always|no`, `start-limit:N/T`.

## `svcd` — a PID-1-like root

The kernel boot spawns `svcd` directly (like `Shell::launch_uterm` spawns the
terminal), granting it full-root FS + FS/PROC brokers + a privileged (can_kill) PROC +
the Nexus admin handle. Everything a service reaches, it reaches because `svcd`
narrowed a capability for it; the machine root is never ambient. `svcd` reads the
enabled set from `/local/registry/services/`, unions with manifests, topo-sorts, and
per service: ensures `/local/state/<svc>` + `/tmp/<svc>`, mints a full-root FS conn and
`OP_OPEN_DIR`-narrows it to those two dirs, and spawns via `loader::spawn_with_grants`
with `TAG_FS` (= state jail, the service's `/`), `TAG_FS_SCRATCH`, a read-only PROC, a
Nexus client, and (optional) config. It retains the child's proc + main handles.

## Lifecycle

One `wait_many` over every live proc handle (`SIG_EXITED`) + control channels. Reap →
consult `restart:`. Exponential backoff (1s…30s, reset after a healthy interval);
`start-limit:N/T` gives up on a crash loop. Readiness = Nexus registration present;
liveness = proc alive; a crash flips both. Stop = drop the child's main handle
(cooperative `PEER_CLOSED`), then privileged kill after a grace window; dropping the
narrowed state/scratch grants revokes data access.

## Enable / disable / mask

One word per service in `/local/registry/services/<name>`: `enabled` | `disabled`
(may still be pulled in by a `requires:`) | `masked` (blocks even pull-in). Absent =
unconfigured (treated as disabled). Verbs go through a `svc` CLI over `svcd`'s control
channel (svcd publishes `"svcd"` on the Nexus) — single writer, immediate action.

## Userless service data

System services have no user, so "app data → running user's apps.data" cannot apply.
Each service is jailed to `/local/state/<svc>` (durable: docroot, DB dir, spool) +
`/tmp/<svc>` (ephemeral). The service literally cannot name `/users/*` (login never
minted it that cap) or another service's state.

## System vs per-user

`svcd` (kernel-spawned) runs system services from `/local/registry/services`;
**`sessiond`** (spawned by login, holding only the `/users/<u>` cap) runs per-user
autostart from `/users/<u>/registry/session/` — same loop, same backoff, isolated by
capability, publishing into a per-user Nexus view so two logged-in users don't collide.

## Decisions

- Sidecar plain-text service manifest (svcd plans without loading ELFs).
- Nexus named broker for readiness/ordering (in-kernel first, later `nexusd`).
- Two FS grants: durable state + ephemeral scratch.
- Enable/disable via svcd control channel (serialized, immediate).

## Tree & surface

`/system/services/*.manifest`, `/local/services/*.manifest`,
`/local/registry/services/<name>`, `/local/state/<svc>`, `/tmp/<svc>`,
`/users/<u>/registry/session/<name>`. New `crates/abi/src/nexus.rs`
(`OP_PUBLISH/LOOKUP/LIST`); tags 7 `NEXUS`, 14 `FS_SCRATCH`, 15 `FS_CONFIG`; kernel
`Nexus` server (sibling of `FsServer`/`ProcServer`); new binaries `svcd`, `sessiond`,
`svc`.
