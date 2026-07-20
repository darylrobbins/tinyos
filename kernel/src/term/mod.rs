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
    /// Background apps (`run <name> &`): output interleaves into scrollback
    /// prefixed with the app name; input stays with the shell/foreground.
    bg_jobs: Vec<RunningApp>,
    /// Visible rows in cells (from the hosting card), for OP_RESIZE.
    rows: usize,
    /// True when the foreground app is the userspace shell `/apps/sh` (the
    /// default). It is re-launched when it exits. False = in-kernel fallback
    /// interpreter (sh unavailable, e.g. diskless boot or non-aarch64).
    shell_session: bool,
}

struct RunningApp {
    name: String,
    process: alloc::sync::Arc<crate::obj::process::Process>,
    thread_id: u32,
    console: alloc::sync::Arc<crate::obj::channel::ChannelEnd>,
    /// Partial line accumulated from console WRITE bytes (flushed on '\n').
    /// While non-empty in LINES mode it doubles as the input prompt, so
    /// `print!("name? ")` + `read_line()` compose like a real terminal.
    partial: String,
    /// Color of the current partial line (OP_WRITE = FG, OP_WRITE_STYLED sets it).
    partial_color: u32,
    /// Explicit colored prompt (OP_SET_PROMPT); overrides the partial-as-prompt
    /// fallback. Empty = use the partial. Lets the shell render a Meridian prompt.
    prompt_spans: Vec<(String, u32)>,
    /// Console protocol input mode (abi::console::INPUT_MODE_*).
    input_mode: u32,
    /// Last (cols, rows) sent as OP_RESIZE; (0, 0) forces the initial send.
    sent_size: (usize, usize),
    /// The shell's current foreground child thread (0 = none), reported via
    /// OP_SET_FOREGROUND. Ctrl+C kills this so a hung child dies but the
    /// shell survives.
    foreground_tid: u32,
    /// Full-screen text surface, when the app has one open.
    surface: Option<AppSurface>,
    /// Bottom-pinned live region (lines keep scrolling above it). Mutually
    /// exclusive with `surface`; opening one closes the other.
    live: Option<AppSurface>,
}

/// A hosted full-screen cell surface (console protocol SURFACE_*). The cell
/// data is snapshotted from the app's MemObj on PRESENT — the address comes
/// from the kernel-side object, never from message contents — so the display
/// is tear-free and the app can't point the terminal at arbitrary memory.
struct AppSurface {
    mem: alloc::sync::Arc<crate::obj::memobj::MemObj>,
    cols: usize,
    rows: usize,
    cells: Vec<abi::console::Cell>,
    /// (row, col, shape, visible) from OP_SURFACE_CURSOR.
    cursor: (usize, usize, u32, bool),
}

impl AppSurface {
    /// Copy the damage rect from the app's shared memory into the snapshot.
    fn snap(&mut self, x: usize, y: usize, w: usize, h: usize) {
        let bytes = unsafe {
            core::slice::from_raw_parts(self.mem.pa() as *const u8, self.mem.size())
        };
        let (x1, y1) = ((x + w).min(self.cols), (y + h).min(self.rows));
        for row in y.min(self.rows)..y1 {
            for col in x.min(self.cols)..x1 {
                let i = row * self.cols + col;
                let o = i * 16;
                let f = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
                self.cells[i] = abi::console::Cell {
                    glyph: f(o),
                    fg: f(o + 4),
                    bg: f(o + 8),
                    attrs: u16::from_le_bytes(bytes[o + 12..o + 14].try_into().unwrap()),
                    _pad: 0,
                };
            }
        }
    }
}

impl Terminal {
    pub fn new() -> Self {
        let mut t = Self {
            view: TextView::console(SCROLLBACK, FG, argb(210, 228, 228, 236)),
            history: Vec::new(),
            hist_idx: None,
            // Interactive sessions start in the (single, pre-login) user's home,
            // not the machine root. Login will set this per session.
            cwd: "/users/user".to_string(),
            running: None,
            bg_jobs: Vec::new(),
            rows: 24,
            shell_session: false,
        };
        // Default: run the userspace shell. Fall back to the in-kernel
        // command interpreter only if it can't be launched.
        if !t.launch_shell() {
            t.refresh_prompt();
            t.out(
                format!("tinyOS {VERSION} - userspace shell unavailable, using builtin"),
                DIM,
            );
        }
        t
    }

    /// True while a foreground or background app is alive: the hosting card
    /// reports this as `wants_frames` so the shell keeps a steady frame clock
    /// and pumps the app's console (which the app can fill faster than one
    /// frame drains) instead of deep-idling until the next input event.
    pub fn is_hosting(&self) -> bool {
        self.running.is_some() || !self.bg_jobs.is_empty()
    }

    /// Size in cells; the hosting card updates this from its rect.
    pub fn set_size(&mut self, cols: usize, rows: usize) {
        self.view.cols = cols;
        self.rows = rows;
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
        crate::smoke::mirror(&s);
        self.view.append_frozen(s, color);
    }

    pub fn on_char(&mut self, c: char) {
        if self.running.is_some() {
            return self.app_char(c);
        }
        if c == '\n' {
            self.execute();
        } else {
            self.view.insert_char(c);
            self.hist_idx = None;
        }
    }

    pub fn on_key(&mut self, code: u16) {
        if self.running.is_some() {
            return self.app_key(code);
        }
        match code {
            keys::BACKSPACE => self.view.backspace(),
            keys::LEFT => self.view.left(),
            keys::RIGHT => self.view.right(),
            keys::UP => self.history_nav(true),
            keys::DOWN => self.history_nav(false),
            _ => {}
        }
    }

    /// Character input while an app runs: raw OP_CHAR in KEYS mode; line
    /// editing + OP_INPUT_LINE on Enter in LINES mode.
    fn app_char(&mut self, c: char) {
        use abi::console::{INPUT_MODE_KEYS, OP_CHAR, OP_INPUT_LINE};
        // An open surface implies raw input regardless of the stored mode.
        let raw = self
            .running
            .as_ref()
            .map(|a| a.input_mode == INPUT_MODE_KEYS || a.surface.is_some())
            .unwrap_or(false);
        if raw {
            let mut b = OP_CHAR.to_le_bytes().to_vec();
            b.extend_from_slice(&(c as u32).to_le_bytes());
            self.send_app(b);
            return;
        }
        if c == '\n' {
            let text = self.view.active_text();
            let app = self.running.as_mut().unwrap();
            // Echo the app's pending prompt + what was typed, like a tty.
            let prompt_text: String = if !app.prompt_spans.is_empty() {
                app.prompt_spans.iter().map(|(t, _)| t.as_str()).collect()
            } else {
                app.partial.clone()
            };
            let echo = format!("{prompt_text}{text}");
            app.partial.clear();
            self.view.freeze_active_as(echo, DIM);
            let mut b = OP_INPUT_LINE.to_le_bytes().to_vec();
            b.extend_from_slice(text.as_bytes());
            self.send_app(b);
        } else {
            self.view.insert_char(c);
        }
    }

    /// Key input while an app runs. The compositor only delivers key-down
    /// edges here, so KEYS mode reports down=1 for every event.
    fn app_key(&mut self, code: u16) {
        use abi::console::{INPUT_MODE_KEYS, OP_KEY};
        let raw = self
            .running
            .as_ref()
            .map(|a| a.input_mode == INPUT_MODE_KEYS || a.surface.is_some())
            .unwrap_or(false);
        if raw {
            let mut b = OP_KEY.to_le_bytes().to_vec();
            b.extend_from_slice(&code.to_le_bytes());
            b.push(1); // down
            b.push(0); // mods
            self.send_app(b);
            return;
        }
        match code {
            keys::BACKSPACE => self.view.backspace(),
            keys::LEFT => self.view.left(),
            keys::RIGHT => self.view.right(),
            _ => {}
        }
    }

    /// Ctrl+C: interrupt. Kills the shell's hung foreground child if one is
    /// registered (the shell survives and re-prompts); in the fallback
    /// interpreter it kills the directly-run app; at a bare shell prompt it
    /// just cancels the current input line.
    pub fn on_ctrl_key(&mut self, code: u16) {
        if code != abi::keys::KEY_C {
            return;
        }
        let (fg_tid, thread_id) = match &self.running {
            Some(a) => (a.foreground_tid, a.thread_id),
            None => return,
        };
        if fg_tid != 0 {
            crate::sched::kill(fg_tid);
            self.out("^C".to_string(), DIM);
        } else if self.shell_session {
            // Bare shell prompt: cancel the typed line, keep the shell.
            self.out("^C".to_string(), DIM);
            self.view.set_active(String::new());
        } else {
            crate::sched::kill(thread_id);
            self.out("^C".to_string(), DIM);
        }
    }

    /// Send a console-protocol message to the running app (best effort).
    fn send_app(&mut self, bytes: Vec<u8>) {
        if let Some(app) = &self.running {
            let _ = app.console.send(crate::obj::channel::Message {
                bytes,
                handles: Vec::new(),
            });
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
                    ("run <name> [&]", "run an app from /apps (& = background)"),
                    ("jobs", "list running apps"),
                    ("ls [path]", "list directory"),
                    ("cat <file>", "print file contents"),
                    ("edit <file>", "edit a file in a new window"),
                    ("vi <file>", "edit a file with the vi editor"),
                    ("write <file> <text>", "write text to a file"),
                    ("append <file> <text>", "append text to a file"),
                    ("touch <file>", "create an empty file"),
                    ("mkdir <dir>", "create a directory"),
                    ("rm [-r] <path>", "remove a file or directory"),
                    ("cp <from> <to>", "copy a file (cp app /apps/name installs it)"),
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
                    for (pid, name, tid, mem) in procs {
                        self.out(
                            format!("{pid:>4}  {name:<8} thread {tid}  {} KiB", mem >> 10),
                            FG,
                        );
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
            // edit is a userspace windowed app now (Phase 4 eviction);
            // spawn it as a background job so the shell stays usable.
            "edit" => {
                let p = rest.trim();
                if p.is_empty() {
                    self.out("usage: edit <file>".to_string(), ERR);
                } else {
                    self.run_app(&format!("edit {p} &"));
                }
            }
            // vi is a userspace terminal app now (Phase 4 eviction).
            "vi" => {
                let p = rest.trim();
                if p.is_empty() {
                    self.out("usage: vi <file>".to_string(), ERR);
                } else {
                    self.run_app(&format!("vi {p}"));
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
            "cp" => match rest.trim().split_once(' ') {
                Some((from, to)) => match crate::fs::read(&self.cwd, from.trim()) {
                    Ok(data) => {
                        if let Err(e) = crate::fs::write(&self.cwd, to.trim(), &data, false) {
                            self.out(format!("cp: {e}"), ERR);
                        }
                    }
                    Err(e) => self.out(format!("cp: {from}: {e}"), ERR),
                },
                None => self.out("usage: cp <from> <to>".to_string(), ERR),
            },
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
            "jobs" => {
                let mut any = false;
                if let Some(app) = &self.running {
                    self.out(
                        format!("{:>4}  {:<10} foreground", app.thread_id, app.name.clone()),
                        FG,
                    );
                    any = true;
                }
                let rows: Vec<(u32, String)> = self
                    .bg_jobs
                    .iter()
                    .map(|j| (j.thread_id, j.name.clone()))
                    .collect();
                for (tid, jname) in rows {
                    self.out(format!("{tid:>4}  {jname:<10} background"), FG);
                    any = true;
                }
                if !any {
                    self.out("no running apps".to_string(), DIM);
                }
            }
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

    /// Load and run an app from /apps/<name> with argv. A trailing `&`
    /// runs it in the background (output prefixed, input stays here).
    /// Launch `/apps/sh` as the terminal's shell. On success it becomes the
    /// foreground app that owns the console; on failure the caller falls back
    /// to the in-kernel command interpreter. aarch64 only.
    fn launch_shell(&mut self) -> bool {
        #[cfg(target_arch = "aarch64")]
        {
            if self.spawn_app("sh", &[], false).is_ok() {
                self.shell_session = true;
                return true;
            }
        }
        false
    }

    /// The in-kernel `run` builtin (fallback shell only). Spawns an app from
    /// /apps, foreground unless a trailing `&`.
    fn run_app(&mut self, args: &str) {
        let (args, background) = match args.trim().strip_suffix('&') {
            Some(rest) => (rest.trim(), true),
            None => (args, false),
        };
        if !background && self.running.is_some() {
            self.out("run: a foreground app is already running".to_string(), ERR);
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
        }
        #[cfg(target_arch = "aarch64")]
        match self.spawn_app(name, &argv, background) {
            Ok(tid) => self.out(format!("run: started {name} (thread {tid})"), DIM),
            Err(e) => self.out(format!("run: {name}: {e}"), ERR),
        }
    }

    /// Spawn `/apps/<name>` with argv, wiring its console/fs/proc services
    /// and window channel. Foreground apps take over the console; background
    /// ones join `bg_jobs`. Returns the thread id.
    #[cfg(target_arch = "aarch64")]
    fn spawn_app(&mut self, name: &str, argv: &[String], background: bool) -> Result<u32, String> {
        use crate::obj::channel::create;
        use crate::obj::handle::{Handle, RIGHTS_ALL};
        use crate::obj::Object;
        use abi::bootstrap::{
            TAG_CONSOLE, TAG_FS, TAG_FS_BROKER, TAG_PROC, TAG_PROC_BROKER, TAG_SHELL,
        };

        let elf = crate::fs::read("/", &format!("/apps/{name}")).map_err(|e| format!("{e}"))?;

        // Console + shell channels: this terminal keeps the kernel ends.
        let (console_app, console_kern) = create();
        let (shell_app, shell_kern) = create();

        // FS/PROC: a fresh isolated connection from the standing servers, plus
        // the broker channels so the child can mint for ITS own children.
        let grants: alloc::vec::Vec<(u32, Handle)> = alloc::vec![
            (TAG_CONSOLE, Handle::new(Object::Channel(console_app), RIGHTS_ALL)),
            (TAG_SHELL, Handle::new(Object::Channel(shell_app), RIGHTS_ALL)),
            (TAG_FS, crate::svc::mint_fs()),
            (TAG_PROC, crate::svc::mint_proc()),
            (TAG_FS_BROKER, crate::svc::fs_broker_handle()),
            (TAG_PROC_BROKER, crate::svc::proc_broker_handle()),
        ];

        let (process, tid, _main_kern) = crate::obj::loader::spawn_with_grants(
            name.to_string(),
            &elf,
            argv,
            grants,
        )
        .map_err(|e| e.msg())?;

        // Hand the window channel to the compositor. Don't steal focus: you're
        // typing in the terminal, so the app it runs opens unfocused.
        crate::ui::shell::extern_app::register(shell_kern, name.to_string(), false);

        let job = RunningApp {
            name: name.to_string(),
            process,
            thread_id: tid,
            console: console_kern,
            partial: String::new(),
            partial_color: FG,
            prompt_spans: Vec::new(),
            input_mode: abi::console::INPUT_MODE_LINES,
            sent_size: (0, 0), // forces the initial OP_RESIZE
            foreground_tid: 0,
            surface: None,
            live: None,
        };
        if background {
            self.bg_jobs.push(job);
        } else {
            self.running = Some(job);
            self.view.set_prompt(Vec::new()); // the app owns the prompt now
        }
        Ok(tid)
    }

    /// Drain background jobs: WRITE lines land in scrollback prefixed with
    /// the app name; HELLO/RESIZE are answered; surface/live/input-mode
    /// requests are ignored (only the foreground app owns the display).
    fn pump_bg(&mut self) {
        use abi::console::*;
        let (cols, rows) = (self.view.cols, self.rows);
        let mut lines: Vec<(String, u32)> = Vec::new();
        let mut gone: Vec<u32> = Vec::new();
        for app in self.bg_jobs.iter_mut() {
            let mut replies: Vec<Vec<u8>> = Vec::new();
            while let Ok(msg) = app.console.recv() {
                if msg.bytes.len() < 4 {
                    continue;
                }
                match u32::from_le_bytes(msg.bytes[0..4].try_into().unwrap()) {
                    OP_WRITE => {
                        if let Ok(s) = core::str::from_utf8(&msg.bytes[4..]) {
                            for ch in s.chars() {
                                if ch == '\n' {
                                    let line = core::mem::take(&mut app.partial);
                                    lines.push((format!("[{}] {line}", app.name), FG));
                                } else {
                                    app.partial.push(ch);
                                }
                            }
                        }
                    }
                    OP_HELLO => {
                        let mut b = OP_HELLO_ACK.to_le_bytes().to_vec();
                        b.extend_from_slice(&1u32.to_le_bytes());
                        b.extend_from_slice(&0u32.to_le_bytes());
                        replies.push(b);
                    }
                    _ => {}
                }
            }
            if app.sent_size != (cols, rows) {
                app.sent_size = (cols, rows);
                let mut b = OP_RESIZE.to_le_bytes().to_vec();
                b.extend_from_slice(&(cols as u32).to_le_bytes());
                b.extend_from_slice(&(rows as u32).to_le_bytes());
                replies.push(b);
            }
            for b in replies {
                let _ = app.console.send(crate::obj::channel::Message {
                    bytes: b,
                    handles: Vec::new(),
                });
            }
            if let Some(code) = app.process.exited() {
                if !app.partial.is_empty() {
                    let line = core::mem::take(&mut app.partial);
                    lines.push((format!("[{}] {line}", app.name), FG));
                }
                lines.push((
                    format!("[{}] exited (code {code})", app.name),
                    DIM,
                ));
                gone.push(app.thread_id);
            }
        }
        self.bg_jobs.retain(|j| !gone.contains(&j.thread_id));
        for (line, color) in lines {
            self.out(line, color);
        }
    }

    /// Drain a running app's console messages (console protocol v1) and
    /// detect its exit. Called each frame from the hosting card.
    pub fn pump(&mut self) {
        use abi::console::*;
        self.pump_bg();
        let (cols, rows) = (self.view.cols, self.rows);
        let Some(app) = &mut self.running else { return };
        let mut lines: Vec<(String, u32)> = Vec::new();
        let mut replies: Vec<Vec<u8>> = Vec::new();
        let mut clear_screen = false;
        // Drain all queued console messages.
        while let Ok(msg) = app.console.recv() {
            if msg.bytes.len() < 4 {
                continue;
            }
            let op = u32::from_le_bytes(msg.bytes[0..4].try_into().unwrap());
            match op {
                OP_WRITE => {
                    app.partial_color = FG;
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
                OP_WRITE_STYLED if msg.bytes.len() >= 8 => {
                    let fg = u32::from_le_bytes(msg.bytes[4..8].try_into().unwrap());
                    app.partial_color = fg;
                    if let Ok(s) = core::str::from_utf8(&msg.bytes[8..]) {
                        for ch in s.chars() {
                            if ch == '\n' {
                                lines.push((core::mem::take(&mut app.partial), fg));
                            } else {
                                app.partial.push(ch);
                            }
                        }
                    }
                }
                OP_CLEAR => clear_screen = true,
                OP_SET_PROMPT if msg.bytes.len() >= 8 => {
                    let b = &msg.bytes;
                    let count = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
                    let mut spans = Vec::with_capacity(count);
                    let mut o = 8usize;
                    for _ in 0..count {
                        let (Some(fg), Some(len)) = (
                            b.get(o..o + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap())),
                            b.get(o + 4..o + 8).map(|c| u32::from_le_bytes(c.try_into().unwrap()) as usize),
                        ) else {
                            break;
                        };
                        o += 8;
                        let Some(txt) = b.get(o..o + len).and_then(|s| core::str::from_utf8(s).ok())
                        else {
                            break;
                        };
                        spans.push((txt.to_string(), fg));
                        o += len;
                    }
                    app.prompt_spans = spans;
                }
                OP_HELLO => {
                    let mut b = OP_HELLO_ACK.to_le_bytes().to_vec();
                    b.extend_from_slice(&1u32.to_le_bytes()); // protocol v1
                    b.extend_from_slice(&0u32.to_le_bytes()); // features: none yet
                    replies.push(b);
                }
                OP_SET_INPUT_MODE if msg.bytes.len() >= 8 => {
                    app.input_mode =
                        u32::from_le_bytes(msg.bytes[4..8].try_into().unwrap());
                    // A mode switch marks a new interaction phase — e.g. a
                    // console app launched from a userspace shell that shares
                    // the console, which missed the one-shot startup Resize.
                    // Re-arm it so the app learns the terminal dimensions.
                    app.sent_size = (0, 0);
                    // LINES mode and a full-screen surface are mutually
                    // exclusive. A shell returns to LINES after each child, so
                    // this tears down a surface a crashed/killed child left
                    // behind (it never sent SURFACE_CLOSE) — self-healing.
                    if app.input_mode == INPUT_MODE_LINES {
                        app.surface = None;
                        app.live = None;
                    }
                }
                OP_SET_FOREGROUND if msg.bytes.len() >= 8 => {
                    app.foreground_tid =
                        u32::from_le_bytes(msg.bytes[4..8].try_into().unwrap());
                }
                OP_SURFACE_OPEN if msg.bytes.len() >= 12 => {
                    let scols = u32::from_le_bytes(msg.bytes[4..8].try_into().unwrap()) as usize;
                    let srows = u32::from_le_bytes(msg.bytes[8..12].try_into().unwrap()) as usize;
                    // The surface rides as the first moved MemObj handle; its
                    // address/size come from the kernel object only.
                    let mem = msg.handles.iter().find_map(|h| match &h.object {
                        crate::obj::Object::MemObj(m) => Some(m.clone()),
                        _ => None,
                    });
                    if let Some(mem) = mem {
                        let ok = (1..=1000).contains(&scols)
                            && (1..=500).contains(&srows)
                            && mem.size() >= scols * srows * 16;
                        if ok {
                            let mut s = AppSurface {
                                mem,
                                cols: scols,
                                rows: srows,
                                cells: alloc::vec![abi::console::Cell::default(); scols * srows],
                                cursor: (0, 0, 0, false),
                            };
                            s.snap(0, 0, scols, srows);
                            app.live = None; // exclusive with the live region
                            app.surface = Some(s);
                            // Spec: RESIZE is (re)sent after any open.
                            app.sent_size = (0, 0);
                        }
                    }
                }
                OP_SURFACE_PRESENT if msg.bytes.len() >= 20 => {
                    if let Some(s) = app.surface.as_mut().or(app.live.as_mut()) {
                        let f = |o: usize| {
                            u32::from_le_bytes(msg.bytes[o..o + 4].try_into().unwrap()) as usize
                        };
                        s.snap(f(4), f(8), f(12), f(16));
                    }
                }
                OP_SURFACE_CURSOR if msg.bytes.len() >= 20 => {
                    if let Some(s) = app.surface.as_mut().or(app.live.as_mut()) {
                        let f = |o: usize| {
                            u32::from_le_bytes(msg.bytes[o..o + 4].try_into().unwrap())
                        };
                        s.cursor = (f(4) as usize, f(8) as usize, f(12), f(16) != 0);
                    }
                }
                OP_SURFACE_CLOSE => {
                    app.surface = None;
                    // A surface app (vi, top) launched from a shell that
                    // shares the console has finished; restore line mode so
                    // the shell's prompt reads work again.
                    app.input_mode = INPUT_MODE_LINES;
                }
                OP_LIVE_OPEN | OP_LIVE_RESIZE if msg.bytes.len() >= 8 => {
                    let lrows =
                        u32::from_le_bytes(msg.bytes[4..8].try_into().unwrap()) as usize;
                    let mem = msg.handles.iter().find_map(|h| match &h.object {
                        crate::obj::Object::MemObj(m) => Some(m.clone()),
                        _ => None,
                    });
                    if let Some(mem) = mem {
                        // Region width = terminal width at open time.
                        let ok = (1..=16).contains(&lrows)
                            && cols >= 1
                            && mem.size() >= cols * lrows * 16;
                        if ok {
                            let mut s = AppSurface {
                                mem,
                                cols,
                                rows: lrows,
                                cells: alloc::vec![
                                    abi::console::Cell::default();
                                    cols * lrows
                                ],
                                cursor: (0, 0, 0, false),
                            };
                            s.snap(0, 0, cols, lrows);
                            app.surface = None; // exclusive with full screen
                            app.live = Some(s);
                        }
                    }
                }
                OP_LIVE_CLOSE => {
                    // Flatten the final frame into scrollback.
                    if let Some(s) = app.live.take() {
                        lines.extend(flatten_cells(&s));
                    }
                }
                _ => {}
            }
        }
        // Size notification: once after spawn, then on every change.
        if app.sent_size != (cols, rows) {
            app.sent_size = (cols, rows);
            let mut b = OP_RESIZE.to_le_bytes().to_vec();
            b.extend_from_slice(&(cols as u32).to_le_bytes());
            b.extend_from_slice(&(rows as u32).to_le_bytes());
            replies.push(b);
        }
        for b in replies {
            let _ = app.console.send(crate::obj::channel::Message {
                bytes: b,
                handles: Vec::new(),
            });
        }
        let exited = app.process.exited();
        // LINES-mode prompt: an explicit OP_SET_PROMPT (colored Meridian
        // prompt) wins; otherwise unterminated output doubles as the prompt
        // (`print!("name? ")` reads like a tty).
        let prompt = (app.input_mode == INPUT_MODE_LINES && exited.is_none()).then(|| {
            if !app.prompt_spans.is_empty() {
                app.prompt_spans.clone()
            } else if app.partial.is_empty() {
                Vec::new()
            } else {
                alloc::vec![(app.partial.clone(), app.partial_color)]
            }
        });
        // app borrow ends above; scrollback edits follow.
        if clear_screen {
            self.view.clear();
        }
        for (line, color) in lines {
            self.out(line, color);
        }
        if let Some(spans) = prompt {
            self.view.set_prompt(spans);
        }
        if let Some(code) = exited {
            // Flatten a live region's last frame, flush any trailing partial
            // line, then report.
            let app = self.running.take().unwrap();
            if let Some(s) = &app.live {
                for (line, color) in flatten_cells(s) {
                    self.out(line, color);
                }
            }
            if !app.partial.is_empty() {
                self.out(app.partial, FG);
            }
            if self.shell_session {
                // The shell itself exited (`exit`) — a terminal always wants
                // one, so relaunch. If that now fails, drop to the builtin.
                if !self.launch_shell() {
                    self.shell_session = false;
                    self.refresh_prompt();
                }
            } else {
                self.out(format!("[{}] exited (code {code})", app.thread_id), DIM);
                self.refresh_prompt();
            }
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
        if let Some(s) = self.running.as_ref().and_then(|a| a.surface.as_ref()) {
            return draw_cells(s, surface, fonts, ox, oy, self.view.cols, rows, now_ms);
        }
        // Live region: pinned to the bottom, scrollback keeps the rest.
        let live_rows = self
            .running
            .as_ref()
            .and_then(|a| a.live.as_ref())
            .map(|s| s.rows.min(rows.saturating_sub(1)))
            .unwrap_or(0);
        self.view
            .draw(surface, fonts, ox, oy, rows - live_rows, now_ms, true);
        if live_rows > 0 {
            let s = self.running.as_ref().unwrap().live.as_ref().unwrap();
            let ly = oy + (rows - live_rows) as i32 * CELL_H;
            draw_cells(s, surface, fonts, ox, ly, self.view.cols, live_rows, now_ms);
        }
    }
}

/// Flatten a cell surface into plain scrollback lines (glyphs only; per-cell
/// colors collapse to the default, trailing blanks trimmed).
fn flatten_cells(s: &AppSurface) -> Vec<(String, u32)> {
    use abi::console::ATTR_WIDE_CONT;
    let mut out = Vec::with_capacity(s.rows);
    for row in 0..s.rows {
        let mut line = String::new();
        for col in 0..s.cols {
            let cell = s.cells[row * s.cols + col];
            if cell.attrs & ATTR_WIDE_CONT != 0 {
                continue;
            }
            match char::from_u32(cell.glyph).filter(|c| *c != '\0') {
                Some(c) => line.push(c),
                None => line.push(' '),
            }
        }
        out.push((line.trim_end().to_string(), FG));
    }
    out
}

/// Render a hosted cell surface. Colors with a zero alpha byte take the
/// terminal theme defaults; INVERSE swaps fg/bg; DIM halves the fg;
/// UNDERLINE draws a baseline rule. WIDE glyphs draw once and their
/// continuation cell is skipped.
fn draw_cells(
    s: &AppSurface,
    out: &mut Surface,
    fonts: &mut Fonts,
    ox: i32,
    oy: i32,
    cols: usize,
    rows: usize,
    now_ms: u64,
) {
    use abi::console::*;
    const FONT_PX: f32 = 15.0;
    const DEFAULT_BG: u32 = rgb(0x07, 0x09, 0x0d);
    let dim = |c: u32| (c >> 1) & 0x007F_7F7F | 0xFF00_0000;

    for row in 0..rows.min(s.rows) {
        let y = oy + row as i32 * CELL_H;
        for col in 0..cols.min(s.cols) {
            let cell = s.cells[row * s.cols + col];
            if cell.attrs & ATTR_WIDE_CONT != 0 {
                continue;
            }
            let x = ox + col as i32 * CELL_W;
            let mut fg = if cell.fg >> 24 == 0 { FG } else { cell.fg };
            let mut bg = if cell.bg >> 24 == 0 { None } else { Some(cell.bg) };
            if cell.attrs & ATTR_INVERSE != 0 {
                let old_fg = fg;
                fg = bg.unwrap_or(DEFAULT_BG);
                bg = Some(old_fg);
            }
            if cell.attrs & ATTR_DIM != 0 {
                fg = dim(fg);
            }
            let w = if cell.attrs & ATTR_WIDE != 0 { 2 } else { 1 };
            if let Some(b) = bg {
                out.fill_rect(x, y, CELL_W * w, CELL_H, b);
            }
            if let Some(c) = char::from_u32(cell.glyph).filter(|c| !c.is_whitespace() && *c != '\0')
            {
                let mut buf = [0u8; 4];
                fonts.mono.draw(out, c.encode_utf8(&mut buf), FONT_PX, x, y, fg);
            }
            if cell.attrs & ATTR_UNDERLINE != 0 {
                out.fill_rect(x, y + CELL_H - 3, CELL_W * w, 1, fg);
            }
        }
    }

    // Cursor, on the shared caret blink cadence.
    let (crow, ccol, shape, visible) = s.cursor;
    if visible && crow < rows.min(s.rows) && ccol < cols.min(s.cols) && crate::ui::shell::caret_on(now_ms)
    {
        let (x, y) = (ox + ccol as i32 * CELL_W, oy + crow as i32 * CELL_H);
        match shape {
            CURSOR_BAR => out.fill_rect(x, y + 1, 2, CELL_H - 2, ACCENT),
            CURSOR_UNDERLINE => out.fill_rect(x, y + CELL_H - 3, CELL_W, 2, ACCENT),
            _ => out.fill_rect(x, y + 1, CELL_W, CELL_H - 2, ACCENT),
        }
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
