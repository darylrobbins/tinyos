//! svcd — the tinyOS service supervisor, running in userspace (EL0). The
//! kernel boot-spawns it with a full-root FS grant + the FS/PROC brokers. It
//! discovers services declared under /system/services (+ /local/services),
//! spawns the enabled ones with per-service state/scratch jails, hosts the
//! Nexus (a named registry with readiness), and supervises them (restart with
//! backoff + a start-limit give-up).
//!
//! v1 scope: no `svc` CLI, no `mask`, no per-user sessiond, no RO-proc grant.
//! See docs/superpowers/specs/2026-07-20-service-supervisor-design.md.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use abi::bootstrap::{TAG_FS, TAG_FS_SCRATCH, TAG_NEXUS};
use abi::nexus;
use tinyos_app::channel::Channel;
use tinyos_app::process::Child;
use tinyos_app::syscall::{
    syscall2, RIGHTS_ALL, SIG_EXITED, SIG_PEER_CLOSED, SIG_READABLE, SYS_HANDLE_DUP, SYS_LOG,
};
use tinyos_app::wait::{uptime_us, wait_many, WaitItem};
use tinyos_app::{app, entry::Env, fs};

fn log(s: &str) {
    let b = s.as_bytes();
    syscall2(SYS_LOG, b.as_ptr() as u64, b.len() as u64);
}
macro_rules! logf { ($($a:tt)*) => { log(&format!($($a)*)) } }

// ---- service declaration (sidecar manifest) --------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Restart {
    No,
    OnFailure,
    Always,
}

struct Manifest {
    is_service: bool,
    provides: String,
    requires: Vec<String>,
    after: Vec<String>,
    state: bool,
    scratch: bool,
    restart: Restart,
    /// (N, T_secs): give up after N restarts within T seconds.
    start_limit: (u32, u64),
}

/// Parse a plain-text service manifest. Fail-closed, mirroring
/// kernel `loader::manifest`: unknown tokens are ignored; a file without the
/// `service` token is not a service.
fn parse_manifest(text: &str, name: &str) -> Manifest {
    let mut m = Manifest {
        is_service: false,
        provides: name.to_string(),
        requires: Vec::new(),
        after: Vec::new(),
        state: false,
        scratch: false,
        restart: Restart::OnFailure,
        start_limit: (5, 30),
    };
    for tok in text.lines().map(str::trim).filter(|t| !t.is_empty() && !t.starts_with('#')) {
        match tok {
            "service" => m.is_service = true,
            "state" => m.state = true,
            "scratch" => m.scratch = true,
            "config" => {} // reserved (RO /local/config) — not granted in v1
            t => {
                if let Some(v) = t.strip_prefix("provides:") {
                    m.provides = v.to_string();
                } else if let Some(v) = t.strip_prefix("requires:") {
                    m.requires.push(v.to_string());
                } else if let Some(v) = t.strip_prefix("wants:") {
                    // v1: treat wants like a soft dep for ordering only.
                    m.after.push(v.to_string());
                } else if let Some(v) = t.strip_prefix("after:") {
                    m.after.push(v.to_string());
                } else if let Some(v) = t.strip_prefix("restart:") {
                    m.restart = match v {
                        "no" => Restart::No,
                        "always" => Restart::Always,
                        _ => Restart::OnFailure,
                    };
                } else if let Some(v) = t.strip_prefix("start-limit:") {
                    if let Some((n, tt)) = v.split_once('/') {
                        if let (Ok(n), Ok(tt)) = (n.parse::<u32>(), tt.parse::<u64>()) {
                            m.start_limit = (n, tt);
                        }
                    }
                }
            }
        }
    }
    m
}

// ---- runtime state ---------------------------------------------------------

struct Service {
    name: String,
    m: Manifest,
    /// svcd's end of this instance's Nexus channel (the service holds the peer).
    nexus_srv: Channel,
    child: Option<Child>,
    /// Restart timestamps (µs) within the start-limit window.
    starts: Vec<u64>,
    /// Consecutive restart count, for exponential backoff.
    backoff_count: u32,
    /// Earliest µs at which a restart may occur (0 = ready now).
    restart_at: u64,
    /// Gave up: start-limit exceeded, or a fatal spawn error.
    dead: bool,
}

/// The Nexus registry, hosted inside svcd.
struct Nexus {
    /// name -> (endpoint handle owned by svcd, publishing service index).
    published: Vec<(String, u32, usize)>,
    /// Parked lookups awaiting a publish: (service index, name).
    pending: Vec<(usize, String)>,
}

fn le(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap()))
}

fn nexus_reply_status(ch: Channel, status: u32) {
    let mut r = nexus::R_STATUS.to_le_bytes().to_vec();
    r.extend_from_slice(&status.to_le_bytes());
    let _ = ch.send(&r, &[]);
}

fn nexus_reply_lookup(ch: Channel, endpoint: u32) {
    // Hand the consumer a dup of the endpoint (svcd keeps the original).
    let dup = syscall2(SYS_HANDLE_DUP, endpoint as u64, RIGHTS_ALL as u64).value as u32;
    let mut r = nexus::R_LOOKUP.to_le_bytes().to_vec();
    r.extend_from_slice(&nexus::NX_OK.to_le_bytes());
    let _ = ch.send(&r, &[dup]);
}

impl Nexus {
    fn publish(&mut self, svcs: &[Service], from: usize, name: String, endpoint: u32) {
        // Wake any parked lookups for this name first.
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].1 == name {
                let (idx, _) = self.pending.remove(i);
                nexus_reply_lookup(svcs[idx].nexus_srv, endpoint);
            } else {
                i += 1;
            }
        }
        self.published.push((name, endpoint, from));
    }

    fn lookup(&mut self, svcs: &[Service], from: usize, name: String) {
        if let Some((_, ep, _)) = self.published.iter().find(|(n, _, _)| *n == name) {
            nexus_reply_lookup(svcs[from].nexus_srv, *ep);
        } else {
            self.pending.push((from, name));
        }
    }

    /// Drop everything a dead service published (its endpoint handles are gone).
    fn drop_service(&mut self, idx: usize) {
        self.published.retain(|(_, ep, owner)| {
            if *owner == idx {
                let _ = tinyos_app::syscall::syscall1(
                    tinyos_app::syscall::SYS_HANDLE_CLOSE,
                    *ep as u64,
                );
                false
            } else {
                true
            }
        });
        self.pending.retain(|(i, _)| *i != idx);
    }
}

// ---- spawning --------------------------------------------------------------

/// (Re)create the per-service jails + Nexus channel and spawn the service.
/// On success sets `svc.child` and `svc.nexus_srv`.
fn spawn_service(svc: &mut Service) -> Result<(), u32> {
    let name = svc.name.clone();
    // Durable state jail (the service's "/"). mkdir is idempotent; parents
    // (/local/state, /tmp) are seeded.
    let state_path = format!("/local/state/{name}");
    let _ = fs::mkdir(&state_path);
    let state_dir = fs::open_dir(&state_path)?;

    let mut grants: Vec<(u32, u32)> = alloc::vec![(TAG_FS, state_dir.into_handle())];

    if svc.m.scratch {
        let scratch_path = format!("/tmp/{name}");
        let _ = fs::mkdir(&scratch_path);
        if let Ok(d) = fs::open_dir(&scratch_path) {
            grants.push((TAG_FS_SCRATCH, d.into_handle()));
        }
    }

    // Fresh Nexus channel: svcd keeps the server end, the service gets the client.
    let (srv, client) = Channel::create()?;
    grants.push((TAG_NEXUS, client.0));

    let elf = fs::read(&format!("/system/bin/{name}"))?;
    match tinyos_app::process::spawn(&elf, &[name.as_str()], &grants) {
        Ok(child) => {
            svc.nexus_srv = srv;
            svc.child = Some(child);
            logf!("svcd: started {name}");
            Ok(())
        }
        Err(e) => {
            // The client Nexus handle was moved into the failed grant list; the
            // server end is closed on drop of `srv` when we return.
            srv.close();
            Err(e)
        }
    }
}

// ---- discovery + ordering --------------------------------------------------

fn discover() -> Vec<(String, Manifest)> {
    let mut out: Vec<(String, Manifest)> = Vec::new();
    for dir in ["/system/services", "/local/services"] {
        let entries = match fs::list(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for (fname, _kind, _size) in entries {
            let name = match fname.strip_suffix(".manifest") {
                Some(n) => n.to_string(),
                None => continue,
            };
            let text = match fs::read(&format!("{dir}/{fname}")) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(_) => continue,
            };
            let m = parse_manifest(&text, &name);
            if !m.is_service {
                continue;
            }
            // Enabled iff /local/registry/services/<name> reads "enabled".
            let enabled = fs::read(&format!("/local/registry/services/{name}"))
                .ok()
                .map(|b| String::from_utf8_lossy(&b).trim() == "enabled")
                .unwrap_or(false);
            if !enabled {
                logf!("svcd: {name} disabled, skipping");
                continue;
            }
            // /local shadows /system: skip a dup name already discovered.
            if out.iter().any(|(n, _)| *n == name) {
                continue;
            }
            out.push((name, m));
        }
    }
    out
}

/// Order so each service comes after the services it `requires`/`after`
/// (matched by `provides`). Best-effort: a cycle or missing dep falls through
/// in discovery order — the Nexus's blocking lookup still enforces readiness.
fn topo_order(mut svcs: Vec<(String, Manifest)>) -> Vec<(String, Manifest)> {
    let provider = |name: &str, list: &[(String, Manifest)]| -> Option<String> {
        list.iter().find(|(_, m)| m.provides == name).map(|(n, _)| n.clone())
    };
    let mut ordered: Vec<(String, Manifest)> = Vec::new();
    let mut emitted: Vec<String> = Vec::new();
    while !svcs.is_empty() {
        let ready = svcs.iter().position(|(_, m)| {
            m.requires.iter().chain(m.after.iter()).all(|dep| {
                match provider(dep, &svcs).or_else(|| provider(dep, &ordered)) {
                    // dep provided by an already-emitted service (or absent)
                    Some(p) => emitted.contains(&p) || !svcs.iter().any(|(n, _)| *n == p),
                    None => true,
                }
            })
        });
        let idx = ready.unwrap_or(0); // stuck (cycle/missing) → force-emit head
        let item = svcs.remove(idx);
        emitted.push(item.0.clone());
        ordered.push(item);
    }
    ordered
}

// ---- main ------------------------------------------------------------------

fn main(_env: Env) -> i32 {
    log("svcd: started");

    let decls = topo_order(discover());
    let mut svcs: Vec<Service> = Vec::new();
    for (name, m) in decls {
        let mut svc = Service {
            name,
            m,
            nexus_srv: Channel(0),
            child: None,
            starts: Vec::new(),
            backoff_count: 0,
            restart_at: 0,
            dead: false,
        };
        if let Err(e) = spawn_service(&mut svc) {
            logf!("svcd: {}: spawn failed ({e})", svc.name);
            svc.dead = true;
        }
        svcs.push(svc);
    }

    if svcs.is_empty() {
        log("svcd: no services enabled; idle");
    }

    let mut nx = Nexus { published: Vec::new(), pending: Vec::new() };

    // Supervise loop: wait on child exits + Nexus traffic, honoring restart
    // backoff deadlines.
    loop {
        // Build the wait set + a parallel meta map.
        let mut items: Vec<WaitItem> = Vec::new();
        let mut meta: Vec<(usize, bool)> = Vec::new(); // (svc idx, is_proc)
        let mut next_deadline = u64::MAX;
        for (i, s) in svcs.iter().enumerate() {
            if let Some(c) = &s.child {
                items.push(WaitItem { handle: c.proc_h, want: SIG_EXITED, observed: 0 });
                meta.push((i, true));
                items.push(WaitItem {
                    handle: s.nexus_srv.0,
                    want: SIG_READABLE | SIG_PEER_CLOSED,
                    observed: 0,
                });
                meta.push((i, false));
            } else if !s.dead && s.restart_at != 0 {
                next_deadline = next_deadline.min(s.restart_at);
            }
        }
        if items.is_empty() && next_deadline == u64::MAX {
            // Nothing to supervise and nothing pending — park.
            let _ = wait_many(&mut [], uptime_us() + 5_000_000);
            // Re-check in case a restart timer was armed elsewhere (none here).
            if svcs.iter().all(|s| s.child.is_none() && (s.dead || s.restart_at == 0)) {
                continue;
            }
        }

        let _ = wait_many(&mut items, next_deadline);

        // 1) Service Nexus traffic.
        for (mi, it) in items.iter().enumerate() {
            let (i, is_proc) = meta[mi];
            if is_proc {
                continue;
            }
            if it.observed & SIG_READABLE == 0 {
                continue;
            }
            let ch = svcs[i].nexus_srv;
            while let Ok(msg) = ch.try_recv() {
                match le(&msg.bytes, 0) {
                    Some(op) if op == nexus::OP_PUBLISH => {
                        let nlen = le(&msg.bytes, 4).unwrap_or(0) as usize;
                        let name = String::from_utf8_lossy(
                            msg.bytes.get(8..8 + nlen).unwrap_or(&[]),
                        )
                        .into_owned();
                        if let Some(ep) = msg.handles.first().copied() {
                            nexus_reply_status(ch, nexus::NX_OK);
                            logf!("svcd: {} published {name}", svcs[i].name);
                            nx.publish(&svcs, i, name, ep);
                        } else {
                            nexus_reply_status(ch, nexus::NX_INVALID);
                        }
                    }
                    Some(op) if op == nexus::OP_LOOKUP => {
                        let nlen = le(&msg.bytes, 4).unwrap_or(0) as usize;
                        let name = String::from_utf8_lossy(
                            msg.bytes.get(8..8 + nlen).unwrap_or(&[]),
                        )
                        .into_owned();
                        nx.lookup(&svcs, i, name);
                    }
                    _ => {}
                }
            }
        }

        // 2) Child exits → restart policy.
        let now = uptime_us();
        for (mi, it) in items.iter().enumerate() {
            let (i, is_proc) = meta[mi];
            if !is_proc || it.observed & SIG_EXITED == 0 {
                continue;
            }
            if let Some(c) = svcs[i].child.take() {
                c.release();
            }
            svcs[i].nexus_srv.close();
            nx.drop_service(i);
            let name = svcs[i].name.clone();
            match svcs[i].m.restart {
                Restart::No => {
                    logf!("svcd: {name} exited (no restart)");
                    svcs[i].dead = true;
                }
                _ => {
                    // start-limit: give up after N restarts within T seconds.
                    let (n, t) = svcs[i].m.start_limit;
                    let win = t.saturating_mul(1_000_000);
                    svcs[i].starts.retain(|&ts| now.saturating_sub(ts) < win);
                    svcs[i].starts.push(now);
                    if svcs[i].starts.len() as u32 > n {
                        logf!("svcd: {name} crash-looping; giving up");
                        svcs[i].dead = true;
                    } else {
                        let c = svcs[i].backoff_count.min(5);
                        let backoff = (1_000_000u64 << c).min(30_000_000);
                        svcs[i].backoff_count += 1;
                        svcs[i].restart_at = now + backoff;
                        logf!("svcd: {name} exited; restart in {}ms", backoff / 1000);
                    }
                }
            }
        }

        // 3) Fire due restarts.
        let now = uptime_us();
        for i in 0..svcs.len() {
            if svcs[i].child.is_none() && !svcs[i].dead && svcs[i].restart_at != 0
                && now >= svcs[i].restart_at
            {
                svcs[i].restart_at = 0;
                if let Err(e) = spawn_service(&mut svcs[i]) {
                    logf!("svcd: {}: respawn failed ({e})", svcs[i].name);
                    svcs[i].dead = true;
                }
            }
        }
    }
}

app!(main);
