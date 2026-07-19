//! Terminal widget + built-in command shell.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arch::timer;
use crate::drivers::input::keys;
use crate::sched;
use crate::sched::thread::Class;
use crate::gfx::font::Fonts;
use crate::gfx::surface::{argb, rgb, Surface};
use crate::ui::textview::TextView;
use crate::{mem, VERSION};

// Monospace cell metrics live with the shared text widget; re-exported so the
// hosting card can size itself in cells.
pub use crate::ui::textview::{CELL_H, CELL_W};

const FG: u32 = rgb(0xe8, 0xec, 0xf2);
const ACCENT: u32 = rgb(0x5f, 0xd4, 0xc4);
const DIM: u32 = rgb(0x5f, 0x68, 0x79);
const ERR: u32 = rgb(0xff, 0x9e, 0x9e);

const PROMPT_USER: &str = "daryl@tinyos";
const PROMPT_CHEVRON: &str = "> ";
const SCROLLBACK: usize = 400;

pub struct Terminal {
    /// Scrollback + editable prompt line, rendered by the shared widget.
    view: TextView,
    history: Vec<String>,
    hist_idx: Option<usize>,
    cwd: String,
    /// A foreground app launched via `run`, if any.
    running: Option<RunningApp>,
    /// An `edit <file>` request for the shell to open an editor window,
    /// as (cwd, path). Drained each frame by `Shell::pump_app_requests`.
    pending_edit: Option<(String, String)>,
    /// A `vi <file>` request for the shell to open a vi window, as (cwd, path).
    pending_vi: Option<(String, String)>,
}

struct RunningApp {
    process: alloc::sync::Arc<crate::obj::process::Process>,
    thread_id: u32,
    console: alloc::sync::Arc<crate::obj::channel::ChannelEnd>,
    /// Partial line accumulated from console WRITE bytes (flushed on '\n').
    partial: String,
}

impl Terminal {
    pub fn new() -> Self {
        let mut t = Self {
            view: TextView::console(SCROLLBACK, FG, argb(210, 228, 228, 236)),
            history: Vec::new(),
            hist_idx: None,
            cwd: "/".to_string(),
            running: None,
            pending_edit: None,
            pending_vi: None,
        };
        t.refresh_prompt();
        t.out(format!("tinyOS {VERSION} - type 'help' to get started"), DIM);
        t
    }

    /// Wrap width in cells; the hosting card updates this from its rect.
    pub fn set_cols(&mut self, cols: usize) {
        self.view.cols = cols;
    }

    /// Take a pending `edit <file>` request, if any (shell drains this).
    pub fn take_pending_edit(&mut self) -> Option<(String, String)> {
        self.pending_edit.take()
    }

    /// Take a pending `vi <file>` request, if any (shell drains this).
    pub fn take_pending_vi(&mut self) -> Option<(String, String)> {
        self.pending_vi.take()
    }

    /// Prompt path segment, spaces included (" / ", " /notes ").
    fn prompt_path(&self) -> String {
        format!(" {} ", self.cwd)
    }

    /// Rebuild the multi-color prompt prefix from the current cwd.
    fn refresh_prompt(&mut self) {
        self.view.set_prompt(alloc::vec![
            (PROMPT_USER.to_string(), ACCENT),
            (self.prompt_path(), DIM),
            (PROMPT_CHEVRON.to_string(), ACCENT),
        ]);
    }

    fn out(&mut self, s: String, color: u32) {
        self.view.append_frozen(s, color);
    }

    pub fn on_char(&mut self, c: char) {
        if c == '\n' {
            self.execute();
        } else {
            self.view.insert_char(c);
            self.hist_idx = None;
        }
    }

    pub fn on_key(&mut self, code: u16) {
        match code {
            keys::BACKSPACE => self.view.backspace(),
            keys::LEFT => self.view.left(),
            keys::RIGHT => self.view.right(),
            keys::UP => self.history_nav(true),
            keys::DOWN => self.history_nav(false),
            _ => {}
        }
    }

    fn history_nav(&mut self, up: bool) {
        if self.history.is_empty() {
            return;
        }
        let idx = match (self.hist_idx, up) {
            (None, true) => Some(self.history.len() - 1),
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), false) => None,
        };
        self.hist_idx = idx;
        let text = idx.map(|i| self.history[i].clone()).unwrap_or_default();
        self.view.set_active(text);
    }

    fn execute(&mut self) {
        let raw = self.view.active_text();
        let cmd = raw.trim().to_string();
        let echo = format!("{PROMPT_USER}{}{PROMPT_CHEVRON}{raw}", self.prompt_path());
        self.view.freeze_active_as(echo, DIM);
        self.hist_idx = None;

        if cmd.is_empty() {
            return;
        }
        self.history.push(cmd.clone());

        let (name, rest) = cmd.split_once(' ').unwrap_or((cmd.as_str(), ""));
        match name {
            "help" => {
                self.out("commands:".to_string(), FG);
                for (c, d) in [
                    ("help", "this list"),
                    ("echo <text>", "print text"),
                    ("clear", "clear the screen"),
                    ("sysinfo", "hardware and kernel info"),
                    ("memstat", "heap usage"),
                    ("uptime", "time since boot"),
                    ("date", "current date and time"),
                    ("spin [n]", "spawn n busy threads on cores 1-3"),
                    ("ps", "list threads and processes"),
                    ("kill <id>", "stop a thread"),
                    ("run <name>", "run an app from /apps"),
                    ("ls [path]", "list directory"),
                    ("cat <file>", "print file contents"),
                    ("edit <file>", "edit a file in a new window"),
                    ("vi <file>", "edit a file with the vi editor"),
                    ("write <file> <text>", "write text to a file"),
                    ("append <file> <text>", "append text to a file"),
                    ("touch <file>", "create an empty file"),
                    ("mkdir <dir>", "create a directory"),
                    ("rm [-r] <path>", "remove a file or directory"),
                    ("mv <from> <to>", "move or rename"),
                    ("cd [dir]", "change directory"),
                    ("pwd", "print working directory"),
                    ("fsinfo", "filesystem usage"),
                    ("shutdown", "sync disk and power off"),
                    ("reboot", "sync disk and restart"),
                    ("about", "about tinyOS"),
                ] {
                    self.out(format!("  {c:<22} {d}"), FG);
                }
            }
            "echo" => self.out(rest.to_string(), FG),
            "clear" => self.view.clear(),
            "sysinfo" => {
                let (used, free) = mem::stats();
                let lines = [
                    format!("tinyOS {VERSION}"),
                    format!("arch:      {} ({})", crate::arch::NAME, crate::arch::boot_privilege()),
                    format!("machine:   {}", crate::arch::MACHINE),
                    format!("display:   {}x{} @ 32bpp", crate::fb_size().0, crate::fb_size().1),
                    format!("heap:      {} MiB used / {} MiB free", used >> 20, free >> 20),
                    format!("uptime:    {}", fmt_uptime()),
                ];
                for l in lines {
                    self.out(l, FG);
                }
            }
            "memstat" => {
                let (used, free) = mem::stats();
                self.out(
                    format!("heap: {} KiB used, {} KiB free", used >> 10, free >> 10),
                    FG,
                );
            }
            "uptime" => self.out(fmt_uptime(), FG),
            "date" => self.out(fmt_date(), FG),
            "about" => {
                self.out("tinyOS - a tiny operating system written in Rust".to_string(), ACCENT);
                self.out("UEFI boot, software-composited GUI, 4 cores,".to_string(), FG);
                self.out("cooperative threads, no processes, no problems.".to_string(), FG);
            }
            "spin" => {
                let n: usize = rest.trim().parse().unwrap_or(1).clamp(1, 16);
                // Cores 1-3 take the load; core 0 keeps the desktop smooth.
                // Single-core machines share core 0 cooperatively.
                let affinity = if sched::online_cpus() > 1 { 0b1110 } else { 0b0001 };
                for _ in 0..n {
                    let id = sched::spawn("spin".to_string(), Class::Normal, affinity, spin_worker);
                    self.out(format!("spawned spin thread {id}"), FG);
                }
            }
            "ps" => {
                self.out(
                    format!("{:>4}  {:<8} {:<8} {:>3}  CLASS", "ID", "NAME", "STATE", "CPU"),
                    DIM,
                );
                for t in sched::snapshot() {
                    let state = format!("{:?}", t.state);
                    self.out(
                        format!(
                            "{:>4}  {:<8} {:<8} {:>3}  {:?}",
                            t.id, t.name, state, t.cpu, t.class
                        ),
                        FG,
                    );
                }
                let procs = crate::obj::process::Process::snapshot();
                if !procs.is_empty() {
                    self.out("processes:".to_string(), DIM);
                    for (pid, name, tid) in procs {
                        self.out(format!("{pid:>4}  {name:<8} thread {tid}"), FG);
                    }
                }
            }
            "kill" => match rest.trim().parse::<u32>() {
                Ok(id) if id == sched::ui_thread_id() => {
                    self.out("kill: refusing to kill the ui thread".to_string(), ERR)
                }
                Ok(id) if sched::kill(id) => self.out(format!("kill: signalled {id}"), FG),
                Ok(id) => self.out(format!("kill: no such thread {id}"), ERR),
                Err(_) => self.out("usage: kill <id>".to_string(), ERR),
            },
            "ls" => {
                let path = if rest.trim().is_empty() { "." } else { rest.trim() };
                match crate::fs::list(&self.cwd, path) {
                    Ok(entries) if entries.is_empty() => {}
                    Ok(entries) => {
                        for e in entries {
                            match e.kind {
                                tinyfs::InodeKind::Dir => {
                                    self.out(format!("{:>10}  {}/", "-", e.name), ACCENT)
                                }
                                _ => self.out(format!("{:>10}  {}", e.size, e.name), FG),
                            }
                        }
                    }
                    Err(e) => self.out(format!("ls: {e}"), ERR),
                }
            }
            "cat" => match crate::fs::read(&self.cwd, rest.trim()) {
                Ok(data) => match core::str::from_utf8(&data) {
                    Ok(text) => {
                        for line in text.lines() {
                            self.out(line.to_string(), FG);
                        }
                    }
                    Err(_) => self.out(format!("cat: binary file ({} bytes)", data.len()), DIM),
                },
                Err(e) => self.out(format!("cat: {e}"), ERR),
            },
            "edit" => {
                let p = rest.trim();
                if p.is_empty() {
                    self.out("usage: edit <file>".to_string(), ERR);
                } else {
                    self.pending_edit = Some((self.cwd.clone(), p.to_string()));
                }
            }
            "vi" => {
                let p = rest.trim();
                if p.is_empty() {
                    self.out("usage: vi <file>".to_string(), ERR);
                } else {
                    self.pending_vi = Some((self.cwd.clone(), p.to_string()));
                }
            }
            "write" | "append" => match rest.trim().split_once(' ') {
                Some((file, text)) => {
                    let append = name == "append";
                    match crate::fs::write(&self.cwd, file, text.as_bytes(), append) {
                        Ok(()) => {}
                        Err(e) => self.out(format!("{name}: {e}"), ERR),
                    }
                }
                None => self.out(format!("usage: {name} <file> <text>"), ERR),
            },
            "touch" => {
                let path = rest.trim();
                if path.is_empty() {
                    self.out("usage: touch <file>".to_string(), ERR);
                } else {
                    // Create if absent; leave an existing file or directory be.
                    match crate::fs::read(&self.cwd, path) {
                        Ok(_) | Err(tinyfs::FsError::IsADir) => {}
                        Err(tinyfs::FsError::NotFound) => {
                            if let Err(e) = crate::fs::write(&self.cwd, path, &[], false) {
                                self.out(format!("touch: {e}"), ERR);
                            }
                        }
                        Err(e) => self.out(format!("touch: {e}"), ERR),
                    }
                }
            }
            "mkdir" => match crate::fs::mkdir(&self.cwd, rest.trim()) {
                Ok(()) => {}
                Err(e) => self.out(format!("mkdir: {e}"), ERR),
            },
            "rm" => {
                let (recursive, path) = match rest.trim().strip_prefix("-r ") {
                    Some(p) => (true, p.trim()),
                    None => (false, rest.trim()),
                };
                if path.is_empty() {
                    self.out("usage: rm [-r] <path>".to_string(), ERR);
                } else if let Err(e) = crate::fs::remove(&self.cwd, path, recursive) {
                    self.out(format!("rm: {e}"), ERR);
                }
            }
            "mv" => match rest.trim().split_once(' ') {
                Some((from, to)) => {
                    if let Err(e) = crate::fs::rename(&self.cwd, from.trim(), to.trim()) {
                        self.out(format!("mv: {e}"), ERR);
                    }
                }
                None => self.out("usage: mv <from> <to>".to_string(), ERR),
            },
            "cd" => {
                let path = if rest.trim().is_empty() { "/" } else { rest.trim() };
                match crate::fs::resolve_dir(&self.cwd, path) {
                    Ok(canon) => {
                        self.cwd = canon;
                        self.refresh_prompt();
                    }
                    Err(e) => self.out(format!("cd: {e}"), ERR),
                }
            }
            "pwd" => {
                let cwd = self.cwd.clone();
                self.out(cwd, FG);
            }
            "fsinfo" | "df" => match crate::fs::stats() {
                Ok(st) => {
                    let block_kib = (tinyfs::BLOCK_SIZE / 1024) as u64;
                    let lines = [
                        format!("tinyfs on /dev/vda, generation {}", st.generation),
                        format!(
                            "blocks:  {} used / {} total ({} KiB / {} KiB)",
                            st.used_blocks,
                            st.total_blocks,
                            st.used_blocks * block_kib,
                            st.total_blocks * block_kib
                        ),
                        format!("inodes:  {} used / {} total", st.inodes_used, st.inodes_total),
                    ];
                    for l in lines {
                        self.out(l, FG);
                    }
                }
                Err(e) => self.out(format!("fsinfo: {e}"), ERR),
            },
            "shutdown" | "poweroff" | "halt" | "reboot" => match crate::fs::sync() {
                Ok(()) => {
                    kprintln!("tinyos: {name}: filesystem synced, going down");
                    if name == "reboot" {
                        crate::arch::reboot()
                    } else {
                        crate::arch::poweroff()
                    }
                }
                // A failed sync is the one case where powering off could
                // lose the device cache: refuse and leave the OS running.
                Err(e) => self.out(format!("{name}: sync failed ({e}), aborting"), ERR),
            },
            "run" => self.run_app(rest.trim()),
            "usertest" => self.usertest(rest.trim()),
            "objtest" => {
                for line in crate::obj::objtest::run() {
                    let color = if line.starts_with("PASS") { FG } else { ERR };
                    self.out(line, color);
                }
            }
            "sudo" => self.out(
                "daryl is not in the sudoers file. This incident will be reported.".to_string(),
                ERR,
            ),
            _ => self.out(format!("command not found: {name}"), ERR),
        }
    }

    /// Load and run an app from /apps/<name> with argv.
    fn run_app(&mut self, args: &str) {
        if self.running.is_some() {
            self.out("run: an app is already running".to_string(), ERR);
            return;
        }
        let mut parts = args.split_whitespace();
        let Some(name) = parts.next() else {
            self.out("usage: run <name> [args...]".to_string(), ERR);
            return;
        };
        let argv: Vec<String> = parts.map(|s| s.to_string()).collect();
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = (name, argv);
            self.out("run: userspace unsupported on this arch".to_string(), ERR);
            return;
        }
        #[cfg(target_arch = "aarch64")]
        {
            let path = format!("/apps/{name}");
            let elf = match crate::fs::read("/", &path) {
                Ok(elf) => elf,
                Err(e) => {
                    self.out(format!("run: {name}: {e}"), ERR);
                    return;
                }
            };
            match crate::obj::loader::spawn(name.to_string(), &elf, &argv) {
                Ok(app) => {
                    self.out(format!("run: started {name} (thread {})", app.thread_id), DIM);
                    // Hand the window channel to the shell: if the app opens a
                    // window, the shell hosts it. Console output stays here.
                    crate::ui::shell::extern_app::register(app.shell, name.to_string());
                    self.running = Some(RunningApp {
                        process: app.process,
                        thread_id: app.thread_id,
                        console: app.console,
                        partial: String::new(),
                    });
                }
                Err(e) => self.out(format!("run: {name}: {}", e.msg()), ERR),
            }
        }
    }

    /// Drain a running app's console output and detect its exit. Called each
    /// frame from the hosting card.
    pub fn pump(&mut self) {
        let Some(app) = &mut self.running else { return };
        const OP_WRITE: u32 = 1;
        let mut lines: Vec<(String, u32)> = Vec::new();
        // Drain all queued console messages.
        while let Ok(msg) = app.console.recv() {
            if msg.bytes.len() < 4 {
                continue;
            }
            let op = u32::from_le_bytes(msg.bytes[0..4].try_into().unwrap());
            if op != OP_WRITE {
                continue;
            }
            if let Ok(s) = core::str::from_utf8(&msg.bytes[4..]) {
                for ch in s.chars() {
                    if ch == '\n' {
                        lines.push((core::mem::take(&mut app.partial), FG));
                    } else {
                        app.partial.push(ch);
                    }
                }
            }
        }
        let exited = app.process.exited();
        for (line, color) in lines {
            self.out(line, color);
        }
        if let Some(code) = exited {
            // Flush any trailing partial line, then report.
            let app = self.running.take().unwrap();
            if !app.partial.is_empty() {
                self.out(app.partial, FG);
            }
            self.out(format!("[{}] exited (code {code})", app.thread_id), DIM);
        }
    }

    /// EL0 smoke test (see obj::usertest). `usertest spin` = unkillable-by-
    /// cooperation EL0 loop, proving timer preemption and `kill`.
    fn usertest(&mut self, arg: &str) {
        match crate::obj::usertest::spawn(arg == "spin") {
            Ok(id) => self.out(format!("usertest: spawned EL0 thread {id}"), FG),
            Err(e) => self.out(format!("usertest: {e}"), ERR),
        }
    }

    pub fn draw(
        &mut self,
        surface: &mut Surface,
        fonts: &mut Fonts,
        ox: i32,
        oy: i32,
        rows: usize,
        now_ms: u64,
    ) {
        self.view.draw(surface, fonts, ox, oy, rows, now_ms, true);
    }
}

/// Busy work in ~10 ms slices with a yield between slices, so cooperative
/// scheduling (and kill) always gets a look-in.
fn spin_worker() {
    loop {
        let t0 = timer::uptime_us();
        while timer::uptime_us() - t0 < 10_000 {
            core::hint::spin_loop();
        }
        sched::yield_now(); // exits here when kill_pending is set
    }
}

fn fmt_uptime() -> String {
    let s = timer::uptime_ms() / 1000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn fmt_date() -> String {
    // Boot pretends it is Fri Jul 17 2026, 9:41 am.
    let total_s = 9 * 3600 + 41 * 60 + timer::uptime_ms() / 1000;
    format!(
        "Fri Jul 17 {:02}:{:02}:{:02} 2026",
        total_s / 3600 % 24,
        total_s / 60 % 60,
        total_s % 60
    )
}
