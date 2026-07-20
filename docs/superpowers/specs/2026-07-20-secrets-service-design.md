# Secrets service — design

Status: design. Part of the capability substrate (see
`2026-07-20-fs-tree-and-substrate-design.md`). Replaces plaintext dotfile
credentials, TLS private keys, WiFi PSKs, browser/chat master keys, API tokens, and
the password-hash store — none of which have a safe home under capability+jail alone.

## Core idea: the oracle

A secret is never a file an app can `open()`. It is vended over a channel by
**`secretd`**, the sole holder of the FS capabilities to the vault dirs and the sole
holder of the decryption keys. The one genuinely new primitive is the **oracle**: a
capability that lets you *use* a secret you can never *read*. `OP_REF{label}` returns
an oracle handle exposing only `OP_VERIFY` (hash+compare inside `secretd`),
`OP_SIGN`, `OP_UNWRAP` — no read op exists. That is structurally why a password hash
or TLS key can be *used* without any process (not even the authenticator)
*exfiltrating* it.

Everything else is the existing FS jail model applied to secrets: authority arrives as
a moved handle; scope only narrows (`OP_SCOPE`, mirroring `OP_OPEN_DIR`); dropping the
channel revokes (hierarchical revocation).

## Processes

- **`secretd`** (`/system/services/`) — owns the machine vault and, once unlocked, the
  per-user vault(s); holds unlocked DEKs in RAM only. Broker-per-connection like
  `FsServer`.
- **`authd`** (`/system/services/`) — the authenticator. Holds the only oracle refs to
  the `auth/<user>` namespace. Exposes a Login protocol; the login UI never sees a hash.

## Protocol (`crates/abi/src/secret.rs`)

Broker handshake reuses `OP_CONNECT → R_CONNECTED` verbatim. On a connection:
`OP_SCOPE` (narrow to a label subset, rides a moved handle — can only narrow),
`OP_GET` (vend value), `OP_PUT{CREATE|ROTATE|GENERATE}`, `OP_REF` (mint oracle),
`OP_LIST`, `OP_DELETE`. On an oracle: `OP_VERIFY`, `OP_SIGN`, `OP_UNWRAP` (no read).
Per-connection `MAX_SCOPES`/`MAX_REFS` like `MAX_SUBDIRS=16`.

## Addressing & manifest

Per-app label namespace delivered as a narrowed capability. Internally every entry is
`<vault>/<app-id>/<label>`; an app cannot express another app's namespace. New
default-deny manifest tokens (parsed like `fs:`): `secret:<label>` (read+use),
`secret.verify:<label>` (oracle only), `secret.admin` (Keychain UI / uninstaller). The
spawner mints a vault connection and `OP_SCOPE`s it to `manifest.secrets ∩ policy`
before granting `TAG_SECRETS` to the child. `auth/*` and the machine vault are never
grantable to ordinary apps — enforced by spawner intersection.

## Storage & unlock

- `/local/secrets/vault` (machine: TLS keys, WiFi PSK, `auth/<user>` hashes),
  `/users/<u>/secrets/vault` (per-user: app master keys, tokens). Reachable only through
  `secretd`'s private FS connections; no manifest may declare `fs:/local/secrets` or
  `fs:/users/<u>/secrets` — the one hard-denied invariant.
- **One AEAD blob per vault** (XChaCha20-Poly1305 over the entry table), a random DEK
  wrapped by a KEK. Atomic replace via tinyfs CoW. Hides label names + secret count.
- **User vault KEK** = Argon2id(login password, salt), derived at login, never stored;
  unlocked at login, DEK zeroized on logout. Binds vault unlock to the same login event
  that mints the `/users/<u>` home capability.

## Boot-before-login (machine secrets)

TLS/WiFi need secrets at boot with no human. **Decision: a sealed-key abstraction,
TPM-ready, backed today by a machine-key blob on the ESP** (`/volumes/boot`,
updater-only). init hands `secretd` a one-shot `TAG_MACHINEKEY` carrying the KEK;
`secretd` only ever sees "a KEK arrived," so a real TPM unseal drops in later with no
code change. **Documented caveat:** until a TPM exists, machine secrets are protected
against process access + casual disk reads, *not* against offline theft of disk+ESP
together.

## Lifecycle

Mint (`OP_PUT{CREATE}`), generate-in-place (`GENERATE` → app gets only an oracle,
ideal for TLS keys), rotate (`ROTATE`, bump version; force-close vs graceful is a
policy knob), revoke (drop the channel; admin revoke reaps the app's connection
subtree), audit (`secret.admin` Keychain UI lists `(app-id, label, right, granted-at)`
from the in-vault ledger), uninstall (`OP_DELETE` over the app's namespace).

## Decisions

- Value-vend **and** oracle, oracle-preferred (`auth/*` oracle-mandatory).
- Separate `authd` (keeps "sole holder of auth caps" crisp).
- Static manifest consent now; runtime powerbox-style prompting later.
- One AEAD blob per vault.
- Machine unlock via sealed-key abstraction / machine-key blob (TPM-ready).

## Tree

`/local/secrets/vault`, `/users/<u>/secrets/vault`, `/volumes/boot` (machine key
blob → TPM handle later), `/system/services/{secretd,authd}`. New tags 8
`SECRETS`, 9 `SECRETS_BROKER`, 17 `MACHINEKEY`.
