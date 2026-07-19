//! Headless service host for launcher-spawned SDK apps: the shell (not a
//! terminal) pumps their fs/proc services and drains console output to the
//! kernel log. Windowed apps register with extern_app as usual; apps that
//! need a real console/text-surface belong in a terminal, not here.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;

use crate::obj::channel::ChannelEnd;
use crate::obj::process::Process;

pub struct SvcJob {
    pub name: String,
    process: Arc<Process>,
    console: Arc<ChannelEnd>,
    fs: crate::fs::service::FsService,
    proc: crate::obj::procsrv::ProcService,
    partial: String,
}

impl SvcJob {
    /// Load /apps/<name> and spawn it as a shell-hosted (windowed) app.
    pub fn spawn(name: &str, argv: &[String]) -> Result<Self, String> {
        let elf = crate::fs::read("/", &format!("/apps/{name}"))
            .map_err(|e| format!("{e}"))?;
        let app = crate::obj::loader::spawn(name.to_string(), &elf, argv)
            .map_err(|e| e.msg())?;
        super::extern_app::register(app.shell, name.to_string());
        // Launcher-spawned apps are jailed to a private data dir: their "/"
        // is /data/<name>, created on first spawn. Only the terminal's `run`
        // (an explicit user action) grants a wider view.
        let jail = format!("/data/{name}");
        let _ = crate::fs::mkdir("/", "/data");
        let _ = crate::fs::mkdir("/", &jail);
        Ok(SvcJob {
            name: name.to_string(),
            process: app.process,
            console: app.console,
            fs: crate::fs::service::FsService::new(app.fs, jail, String::from("/")),
            // Launcher-spawned: read-only proc service (no kill authority).
            proc: crate::obj::procsrv::ProcService::new(app.proc, false),
            partial: String::new(),
        })
    }

    /// Pump the app's services; true once the process has exited.
    pub fn pump(&mut self) -> bool {
        self.fs.pump();
        self.proc.pump();
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
