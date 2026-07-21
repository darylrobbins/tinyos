# Multi-user + admin — design

Status: design. Part of the capability substrate (see
`2026-07-20-fs-tree-and-substrate-design.md`). No root user; authority is expressed
only as capabilities; "admin" is a bundle of fine-grained capabilities a user is
*authorized to acquire*; apps never inherit admin authority.

## Core idea: least *standing* privilege

A capability = a live channel handle to a service's control endpoint; mere possession
authorizes the op, and the service enforces by only acting on a channel it minted. The
central distinction:

- **AUTHORIZED set** — cap *names* in the account record: what a user *may acquire*.
  This is what "admin" means.
- **HELD set** — what a session *holds hot*: at login, **none of the admin caps** —
  only home FS + vault + shell channels.

An admin's session is **cold** by default; being admin is standing *authorization*, not
standing *authority*. Exercising an admin cap is a just-in-time, per-operation,
consented act — acquire one authorized cap, **scope- and time-boxed** (e.g.
"install-packages, this invocation, 5-min lease"), used, then **dropped = revoked**
(hierarchical revocation). A compromised admin session's blast radius is zero until a
user-visible consent mints one scoped, time-boxed lease — versus Unix root being
permanently root.

## The admin capability set (grantable handles, not flags)

Each is a channel to a service's admin-mint broker; the service is the enforcer:
`manage-users` (userd), `install-packages` (pkgd — writes `/local/{bin,apps,services,
share}`), `manage-services` (svcd), `configure-network` (netd), `configure-devices`
(devd), `manage-secrets-admin` (authd — reset *others'* passwords), `view-all-logs`
(logd), `read-any-user` (homesd — mint a jailed FS chan to any `/users/<x>`, audited).
`/system` is read-only/pinned — no admin cap writes it; OS updates go via the updater to
`/volumes/boot`.

## Account database

Two physically separate stores, split by which service holds the cap:
- `/local/registry/accounts/<name>.rec` — identity + authorized cap **names**; reachable
  only via a registry-dir FS cap held by `userd` (rw) and `sessiond` (ro). Schema:
  `name, display, home, enabled, authorized_caps[], created_at, schema`. No secret
  material.
- `/local/secrets/auth/<name>` — password hash + vault-wrap; reachable only by `authd`.

"Who can do X" = read the registry (through the registry cap only). userd can't read
secrets; authd can't read the registry.

## Login → cold session

- **`authd`** — `OP_VERIFY{user,pw} → R_VERIFY{status, [vault handle]}`; reads the hash,
  on success unlocks the per-user vault and returns the `/users/<u>/secrets` channel.
  Login never reads a hash.
- **`sessiond`** — login + policy decision point; init grants it a full-root FS cap, the
  registry ro cap, an authd handle, and every service's admin-mint broker (TCB, like
  init).

Flow: greeter (unprivileged) collects `user`+`pw` → sessiond → authd `OP_VERIFY` → read
`<user>.rec` (refuse if disabled) → mint the **cold** grant set: `TAG_FS` =
`OP_OPEN_DIR("/users/<user>")` (jailed home), `TAG_VAULT`, `TAG_SESSION` (a channel to a
per-session **keyring** holding the authorized name set + the admin-mint brokers) → spawn
the shell with this set. **No `TAG_ADMIN_*` at login.** The keyring is where the
authorized/held split lives: it can mint but holds nothing hot and never mints without an
authority-box consent token; a shell compromise leaks only a request channel.

## Elevation = the authority-box (JIT, scoped, timed, audited)

New manifest token `admincap:<name>` — an app declares it wants a named admin cap;
default-deny (the shell never grants `admin:*`). When such a tool launches:
1. Shell routes the spawn to the **authority-box** (trusted, unspoofable chrome).
2. It checks the request against the session's authorized set (via keyring). Not
   authorized → hard deny (authorized-but-cold and unauthorized look identical to the
   tool). Authorized → consent prompt, scope + time boxed: *"netcfg wants: Configure
   network — this launch only, expires in 5 min. [Grant once] [Deny]."*
3. On Grant: the keyring `OP_CONNECT`s its admin-mint broker → mints a **fresh leased
   child** control channel (TTL + this-invocation scope; hierarchical-revocation
   pattern); the authority-box injects it as the tool's `TAG_ADMIN_*` grant; the grant is
   written to `/local/log` (who/what/cap/scope/TTL/when) — audited.
4. On tool exit or lease expiry the child closes → the service reaps it; the session
   returns to cold.

## Cross-user privacy even from admin

A session's only home handle is a channel jailed to `/users/<self>`; no user/app process
holds a `/users` root. Admin caps are channels to *services*, none an FS root — so an
admin literally cannot *name* `/users/<other>` (`..` clamps). Privacy = the absence of a
handle (no bits, no ACL, no enforcement code). `read-any-user` is a separate, audited
cap (a channel to `homesd` whose `OP_MINT{user}` returns a jailed FS chan and logs every
mint), never implied by any other admin cap.

## First-boot & lifecycle

No root account. init/installer holds every admin-mint broker (it started the services).
At install: userd `OP_USER_ADD` writes the first record with the full authorized bundle
(enabled); authd writes the hash + inits the vault; create the home skeleton. The first
admin authority is stored only as **names**, materialized into hot handles only JIT after
login. Create/delete via userd; grant/revoke authorization edits the authorized set
(effective on next acquisition — nothing hot to claw back); password reset via authd;
disable = `enabled=false`; recovery = installer/recovery boot (no root fallback).

**Vault on admin password reset:** data-loss by default (vault was wrapped by the old
password — admin can't reach your secrets); **per-account opt-in escrow** (a recovery key
sealed in the machine vault, authd-only, lets an admin reset rewrap); **plus an
admin-settable machine policy that can make escrow mandatory** for managed/fleet
machines.

## Decisions

- sessiond holds mint brokers + reads the registry + mints per login (only model that
  fits "capability is the principal").
- Admin caps at rest are registry **names**, re-minted — not sealed bearer tokens.
- Revoke authorization via re-read-on-acquire (cheap: sessions are cold, leases
  self-expire); force-close only if leases can be long.
- A dedicated per-session **keyring** (not the shell) holds mint authority.
- Delegation = an attenuated **leased child** channel (drop = revoke).
- Per-invocation short-TTL leases (no session-sticky sudo-timestamp for admin caps).
- Vault reset: data-loss default + opt-in escrow + admin-mandatable policy.

## Tree & surface

`/local/registry/accounts/<name>.rec`, `/local/secrets/auth/<name>`, `/local/config/*`
(netd/devd), `/local/log` (logd + audit sink), `/users/<u>/secrets`, `/system` read-only
(no admin cap writes it). New services `sessiond`, `authd`, `userd`, `homesd`, `logd`;
tags 10 `VAULT`, 11 `SESSION`, 18–25 `ADMIN_{USERS,PKG,SVC,NET,DEV,SECRETS,LOGS,HOMES}`
+ private-range mint-broker tags; manifest tokens `admincap:<name>`, `vault`; ops on
authd/userd/keyring/homesd per above.
