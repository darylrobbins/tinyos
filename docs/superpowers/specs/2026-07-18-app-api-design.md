# tinyOS App API — Design Spec

Date: 2026-07-18
Status: approved for implementation (Milestone 1)

## Goal

An API for building third-party apps against tinyOS: a small, modern,
capability-based kernel ABI plus an SDK crate, with real hardware isolation
(EL0 on aarch64). Explicitly **not POSIX** — a POSIX compatibility layer can
be built later as a userspace library over these primitives without the kernel
ever growing fork, signals, errno, or NUL-terminated strings.

## Decisions (with the user)

- **Real userspace on aarch64 first.** EL0 + `svc` syscalls + per-process
  address spaces on the HVF dev platform. x86_64 keeps compiling; its ring-3
  backend (SYSCALL/SYSRET) is a later milestone behind the same ABI.
- **Apps load from the ESP FAT volume** at runtime — rebuild an app without
  rebuilding the OS. Long-term a real filesystem replaces this reader.
- **Full API designed up front** (console + windowing). Demo 1 is a console
  app run from the terminal; demo 2 is a windowed app composited by Meridian.

## Design pillars

1. **Handles + rights, no ambient authority.** Every kernel object is
   referenced through a per-process handle table. A process is born with
   exactly one handle (its main channel) and receives every other capability
   explicitly in the bootstrap message. There is no global namespace to root
   around in.
2. **Small syscall surface.** ~13 syscalls: handle ops, channel IPC, one
   unified wait, shared-memory objects, exit, clock, log. Everything else —
   stdio, argv, windowing, future filesystem access — is a protocol spoken
   over channels, so the kernel stays small and the API grows in userspace.
3. **Message passing as the spine.** Channels carry bytes + handles. System
   services (console = the terminal, window server = the Meridian shell)
   are protocol endpoints.
4. **No fork, no signals, no errno.** Spawn takes explicit grants; kill is
   asynchronous termination with an observable `EXITED` signal + exit code;
   every syscall returns a status code; strings are `(ptr, len)` UTF-8.
5. **Async-ready, blocking in the SDK.** Kernel syscalls never block except
   `wait_many`; blocking convenience wrappers live in the SDK.
6. **Versioned ABI.** Syscall numbers are stable once shipped; new ones
   append. Apps carry a `.tinyos_abi` section with their ABI version, checked
   by the loader.

## Kernel objects

| Object  | Purpose | Signals |
|---------|---------|---------|
| Channel | Bidirectional message pipe; messages = bytes + moved handles. Bounded: 64 msgs / 64 KiB per direction. | `READABLE`, `WRITABLE`, `PEER_CLOSED` |
| MemObj  | Shared memory: N frames + size; mappable into a process. | — |
| Process | Address space + handle table + threads; exit state. | `EXITED` |

Handle rights (u32 bits): `READ=1, WRITE=2, DUP=4, TRANSFER=8, MAP=16,
WAIT=32`. `handle_dup` may only narrow rights. Sending a handle over a
channel requires `TRANSFER` and moves it (it leaves the sender's table).

Signals (u32 bits): `READABLE=1, WRITABLE=2, PEER_CLOSED=4, EXITED=8`.

## Syscall ABI v0 (aarch64)

`svc #0`; `x8` = syscall number; args in `x0–x5`; returns `x0` = status,
`x1` = value. Multi-value outputs go through caller-provided pointers.
User pointers are validated against the process's recorded mappings.

Status codes (u32): `0 OK, 1 BAD_HANDLE, 2 WRONG_TYPE, 3 ACCESS_DENIED,
4 INVALID_ARGS, 5 PEER_CLOSED, 6 SHOULD_WAIT, 7 TIMED_OUT, 8 NO_MEMORY,
9 BUFFER_TOO_SMALL, 10 LIMIT_EXCEEDED, 11 NOT_SUPPORTED, 12 KILLED`.

| # | Name | Args (x0..) | Returns |
|---|------|-------------|---------|
| 0 | `log` | buf, len | — (debug serial; always granted) |
| 1 | `handle_close` | h | — |
| 2 | `handle_dup` | h, rights_mask | x1 = new handle |
| 3 | `channel_create` | out `*[u32;2]` | two handles via out |
| 4 | `channel_send` | h, bytes, blen, handles, hcount | — (`SHOULD_WAIT` if full) |
| 5 | `channel_recv` | h, buf, cap, hbuf, hcap, out `*[u32;2]` | lens via out; `SHOULD_WAIT` / `BUFFER_TOO_SMALL` (sizes reported) |
| 6 | `wait_many` | items, count, deadline_us | observed signals written back; count 0 = sleep |
| 7 | `memobj_create` | size | x1 = handle |
| 8 | `memobj_map` | h, offset, len | x1 = vaddr (kernel-chosen) |
| 9 | `memobj_size` | h | x1 = size |
| 10 | `process_exit` | code | no return |
| 11 | `clock_uptime` | — | x1 = µs since boot |
| 12 | `abi_version` | — | x1 = 0 |
| 13–15 | reserved (`process_spawn`, `thread_spawn`, `memobj_unmap`) | | `NOT_SUPPORTED` |

Wait item (repr C): `{ handle: u32, want: u32, observed: u32 }`.
`deadline_us` is absolute uptime µs; `u64::MAX` = wait forever.

**x86_64 (reserved, later milestone):** `syscall`/`sysret`; RAX = sysno,
args RDI, RSI, RDX, R10, R8, R9; returns RAX = status, RDX = value.

## Memory layout (aarch64)

The kernel keeps UEFI's identity map in **TTBR0** (its pages are EL1-only, so
EL0 cannot read or execute kernel memory). Each process owns a **TTBR1**
address space:

- `TCR_EL1.T1SZ=30` → 16 GiB user region at `0xFFFF_FFFC_0000_0000..`
- 4 KiB granule; per-process L1 root; L2/L3 on demand; 8-bit ASID in
  TTBR1[63:56]; user pages `nG=1` → context switch = `msr ttbr1_el1; isb`,
  no TLB flush (full `tlbi aside1is` only at process teardown).
- User pages: `AP=01` (RW) / `AP=11` (RO), `PXN=1` always, `UXN=0` only on
  code pages. W^X enforced by the loader.
- Fixed link base for apps: `0xFFFF_FFFC_0040_0000`. Stack and memobj
  mappings are placed by a per-process bump allocator above the image.

## Program binaries

Static non-PIE **ELF64** (`ET_EXEC`, `EM_AARCH64`), hand-rolled loader
(validate ehdr, iterate `PT_LOAD`, copy, zero BSS, apply permissions from
`p_flags`). Fixed-base costs nothing since every process has its own TTBR1;
PIE/ASLR is a pure loader upgrade later. Apps carry a `.tinyos_abi` section
containing `u32 abi_version`; mismatch → clear load error.

## Bootstrap protocol

At spawn the process has handle 1 = client end of the **main channel**. The
first message waiting on it is the bootstrap record (all integers LE):

```
u32 abi_version
u32 argc, then per arg: u32 len + UTF-8 bytes
u32 grant_count, then per grant: u32 tag  (handles ride the message)
```

Grant tags: `1 = CONSOLE` channel, `2 = SHELL` (window server) channel.

## Protocols (message = u32 LE opcode + payload)

**Console v0** — app→terminal: `WRITE=1 {utf8}`. `READ=2` reserved for stdin.
Exit codes are not a protocol message: the terminal watches the Process
handle's `EXITED` signal and prints `exited (code N)`.

**Window v0** — app→shell: `OPEN=1 {w:u32, h:u32, title: u32-len+utf8}` + one
MemObj handle (BGRA, stride = w); `PRESENT=3 {x,y,w,h damage}`.
shell→app: `OPENED=2 {status:u32}`, `CHAR=16 {c:u32}`,
`KEY=17 {code:u16, down:u8}`, `CLOSE_REQ=18`. One window per connection in
v0. The shell composites directly from the MemObj frames (identity-mapped →
zero-copy present).

## Execution model

- A user thread is a normal scheduler thread whose kernel entry activates the
  process address space and `eret`s to EL0 (SPSR = EL0t, IRQs unmasked,
  SP_EL0 = user stack). Its existing 64 KiB stack becomes the kernel stack;
  traps land on SP_EL1.
- Syscalls run as ordinary Rust on the thread's kernel stack and are
  cooperation points; kernel invariants (no locks across `switch_to`, IRQ
  handlers only set flags) are untouched.
- **Preemption exists only at EL0**: entering user mode arms a 10 ms timer
  slice; the lower-EL IRQ vector saves a full trap frame, acks, and calls
  `yield_now()` — safe because user code holds no kernel locks. EL1 stays
  cooperative.
- `kill` sets the existing `kill_pending`; observed at syscall entry/exit, on
  the EL0 preemption path, and by waking blocked syscalls (`KILLED`). Process
  exit closes the handle table (peers see `PEER_CLOSED`), frees the address
  space, and asserts `EXITED` with the code.

## SDK

`apps/` is a separate cargo workspace targeting `aarch64-unknown-none` with a
fixed-base linker script. The `tinyos-app` crate (`no_std`) provides: raw
syscall stubs + `Status`; `_start` glue that parses the bootstrap record and
calls `fn main(env: Env) -> i32`; a heap allocator over one MemObj; channel,
console (`println!`), and window client libraries; panic → `log` + exit 101.
The Makefile `apps` target copies built ELFs into `esp/apps/`, which the
kernel reads at runtime via virtio-blk (QEMU's vvfat) + a read-only FAT
driver. Terminal built-in: `run <name> [args…]`.

## Out of scope for M1

x86_64 ring 3; `process_spawn` from userspace (only the kernel spawns);
demand paging / CoW; per-object wait queues (global wake is correct, just
noisy); stdin; multiple windows per connection; PIE/ASLR; real frame
allocator (frames come from the identity-mapped heap behind a phys-addr API).
