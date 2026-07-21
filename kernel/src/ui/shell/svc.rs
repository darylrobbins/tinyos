//! Headless service host for launcher-spawned SDK apps: the shell (not a
//! terminal) pumps their fs/proc services and drains console output to the
//! kernel log. Windowed apps register with extern_app as usual; apps that
//! need a real console/text-surface belong in a terminal, not here.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;

use crate::obj::channel::ChannelEnd;
use crate::obj::process::Process;

/// The single implicit user profile until a login system exists. Launcher-
/// spawned apps are jailed under this user's home; login will replace it with
/// the current session's user.
const DEFAULT_USER: &str = "user";

/// Create every level of an absolute path, tolerating already-existing dirs.
/// tinyfs `mkdir` is non-recursive, so the launcher walks the chain itself
/// (e.g. `/users/user/apps.data/edit` → `/users`, `/users/user`, …).
fn mkdir_p(path: &str) {
    let mut acc = String::new();
    for part in path.trim_matches('/').split('/') {
        if part.is_empty() {
            continue;
        }
        acc.push('/');
        acc.push_str(part);
        let _ = crate::fs::mkdir("/", &acc);
    }
}

pub struct SvcJob {
    pub name: String,
    process: Arc<Process>,
    console: Arc<ChannelEnd>,
    fs: Option<crate::fs::service::FsService>,
    proc: Option<crate::obj::procsrv::ProcService>,
    partial: String,
}

impl SvcJob {
    /// Load /system/apps/<name> and spawn it as a shell-hosted (windowed) app,
    /// granting the app's declared caps intersected with launcher policy:
    /// FS only inside /users/<user>/apps.data/<name> or /local/shared/*, proc
    /// without kill. Caps are
    /// least-privilege: an app that declares nothing gets nothing (the loader
    /// fails closed), so a launched app receives exactly what it declared ∩ policy.
    pub fn spawn(name: &str, argv: &[String]) -> Result<Self, String> {
        let elf = crate::fs::read("/", &format!("/system/apps/{name}"))
            .map_err(|e| format!("{e}"))?;
        let m = crate::obj::loader::manifest(&elf);

        // FS policy: "self" (or an explicit path equal to it) means the
        // app's private data dir; /local/shared subtrees are granted on request.
        // Anything else is denied — the powerbox will cover it later.
        //
        // TODO(multi-user): DEFAULT_USER is a single implicit profile until
        // login exists; the session will supply the current user's home.
        let selfdir = format!("/users/{DEFAULT_USER}/apps.data/{name}");
        let mut jail = None;
        for req in &m.fs {
            // Canonicalize before the policy check: the jail string is used
            // verbatim as a path root, so "/shared/../apps" must not pass.
            let target = if req == "self" {
                selfdir.clone()
            } else {
                match tinyfs::path::canonical("/", req) {
                    Ok(p) => p,
                    Err(_) => {
                        kprintln!("{name}: fs grant invalid: {req}");
                        continue;
                    }
                }
            };
            let ok = target == selfdir
                || target == "/local/shared"
                || target.starts_with("/local/shared/");
            if !ok {
                kprintln!("{name}: fs grant denied: {target}");
            } else if jail.is_some() {
                kprintln!("{name}: extra fs grant ignored: {target}");
            } else {
                jail = Some(target);
            }
        }
        if let Some(j) = &jail {
            // tinyfs mkdir is non-recursive: walk the whole parent chain.
            mkdir_p(j);
            if *j == selfdir {
                // Seed the app's private layout: inside its jail the app sees
                // /config /data /cache /state as its own root ("/").
                for sub in ["config", "data", "cache", "state"] {
                    let _ = crate::fs::mkdir("/", &format!("{j}/{sub}"));
                }
            }
        }

        let grants = crate::obj::loader::GrantSet {
            console: m.console,
            window: m.window,
            fs: jail.is_some(),
            proc: m.proc,
        };
        let app = crate::obj::loader::spawn(name.to_string(), &elf, argv, &grants)
            .map_err(|e| e.msg())?;
        super::extern_app::register(app.shell, name.to_string(), true);
        Ok(SvcJob {
            name: name.to_string(),
            process: app.process,
            console: app.console,
            fs: jail.map(|j| {
                crate::fs::service::FsService::new(app.fs, j, String::from("/"))
            }),
            // Launcher-spawned: never kill authority, whatever the manifest
            // asks — proc.kill is advisory.
            proc: m
                .proc
                .then(|| crate::obj::procsrv::ProcService::new(app.proc, false)),
            partial: String::new(),
        })
    }

    /// Pump the app's services; true once the process has exited.
    pub fn pump(&mut self) -> bool {
        if let Some(fs) = &mut self.fs {
            fs.pump();
        }
        if let Some(proc) = &mut self.proc {
            proc.pump();
        }
        // Console is a log sink here: no terminal owns this app.
        while let Ok(msg) = self.console.recv() {
            if msg.bytes.len() < 4 {
                continue;
            }
            if u32::from_le_bytes(msg.bytes[0..4].try_into().unwrap())
                != abi::console::OP_WRITE
            {
                continue;
            }
            if let Ok(s) = core::str::from_utf8(&msg.bytes[4..]) {
                for ch in s.chars() {
                    if ch == '\n' {
                        kprintln!("{}: {}", self.name, core::mem::take(&mut self.partial));
                    } else {
                        self.partial.push(ch);
                    }
                }
            }
        }
        if let Some(code) = self.process.exited() {
            if !self.partial.is_empty() {
                kprintln!("{}: {}", self.name, self.partial);
            }
            kprintln!("{}: exited (code {code})", self.name);
            return true;
        }
        false
    }
}
