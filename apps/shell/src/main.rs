//! sh — the tinyOS shell, running entirely in userspace (EL0). Files over
//! the fs protocol, system/process control over the proc protocol, and
//! `run <name>` via SYS_PROCESS_SPAWN. Full parity with the former in-kernel
//! shell: colored Meridian prompt and output, all builtins, background jobs.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use abi::bootstrap::{TAG_CONSOLE, TAG_FS, TAG_FS_BROKER, TAG_PROC, TAG_PROC_BROKER, TAG_SHELL};
use abi::console::INPUT_MODE_LINES;
use abi::fs::KIND_DIR;
use tinyos_app::process::Child;
use tinyos_app::syscall::{syscall2, RIGHTS_ALL, SYS_HANDLE_DUP};
use tinyos_app::{app, entry, fs, proc, process, Env};

// Meridian palette (from abi::tokens), matching the old kernel shell.
const FG: u32 = abi::tokens::TX;
const ACCENT: u32 = abi::tokens::ACC;
const DIM: u32 = abi::tokens::TX3;
const ERR: u32 = abi::tokens::HUE_RED;

/// Print one colored line to the terminal scrollback.
fn out(fg: u32, s: &str) {
    if let Some(c) = entry::console() {
        c.write_styled(fg, &format!("{s}\n"));
    }
}

fn err(s: &str) {
    out(ERR, s);
}

/// Join + normalize a path against `cwd`; handles ".", "..", absolute.
fn resolve(cwd: &str, path: &str) -> String {
    let mut parts: Vec<&str> = if path.starts_with('/') {
        Vec::new()
    } else {
        cwd.split('/').filter(|s| !s.is_empty()).collect()
    };
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        match seg {
            "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut o = String::from("/");
    o.push_str(&parts.join("/"));
    o
}

fn fmt_hms(total_s: u64) -> String {
    format!("{}:{:02}:{:02}", total_s / 3600, total_s / 60 % 60, total_s % 60)
}

struct Job {
    name: String,
    child: Child,
}

struct Shell {
    env: Env,
    cwd: String,
    jobs: Vec<Job>,
}

impl Shell {
    /// Capabilities a child inherits. Console + shell are shared (dup); FS and
    /// PROC are a FRESH private connection minted per child from the broker, so
    /// siblings and background jobs never share a request/reply channel. The
    /// brokers themselves are forwarded so the child can mint for its children.
    fn child_grants(&self) -> Vec<(u32, u32)> {
        let mut g = Vec::new();
        for (tag, ch) in [(TAG_CONSOLE, self.env.console.0), (TAG_SHELL, self.env.shell.0)] {
            if ch != 0 {
                if let Ok(h) = syscall2(SYS_HANDLE_DUP, ch as u64, RIGHTS_ALL as u64).ok() {
                    g.push((tag, h as u32));
                }
            }
        }
        if self.env.fs_broker.0 != 0 {
            if let Ok(c) = tinyos_app::broker::connect(self.env.fs_broker) {
                g.push((TAG_FS, c.0));
            }
        }
        if self.env.proc_broker.0 != 0 {
            if let Ok(c) = tinyos_app::broker::connect(self.env.proc_broker) {
                g.push((TAG_PROC, c.0));
            }
        }
        for (tag, br) in [
            (TAG_FS_BROKER, self.env.fs_broker.0),
            (TAG_PROC_BROKER, self.env.proc_broker.0),
        ] {
            if br != 0 {
                if let Ok(h) = syscall2(SYS_HANDLE_DUP, br as u64, RIGHTS_ALL as u64).ok() {
                    g.push((tag, h as u32));
                }
            }
        }
        g
    }

    fn run(&mut self, name: &str, args: &[&str], background: bool) {
        let grants = self.child_grants();
        let path = format!("/system/apps/{name}");
        match process::exec(&path, args, &grants, /*want_window=*/ true) {
            Ok(child) => {
                // A detached app (its own window, no console) never touches this
                // console, so holding the prompt on it would just strand a GUI
                // window (you couldn't launch a second app). Auto-background it
                // even without `&`; console apps (vi, top) run foreground.
                if background || child.detached {
                    out(DIM, &format!("[{}] {name} &", child.thread_id));
                    self.jobs.push(Job { name: name.to_string(), child });
                } else {
                    // Register as the foreground child so the terminal's
                    // Ctrl+C kills it (not us) if it hangs.
                    if let Some(c) = entry::console() {
                        c.set_foreground(child.thread_id);
                    }
                    child.wait();
                    // Clear the foreground registration and restore LINES mode
                    // (also tears down any surface a crashed child left behind).
                    if let Some(c) = entry::console() {
                        c.set_foreground(0);
                        c.set_input_mode(INPUT_MODE_LINES);
                    }
                }
            }
            Err(st) => err(&format!("run: {name}: not found or failed (status {st})")),
        }
    }

    /// Reap finished background jobs (called before each prompt).
    fn reap(&mut self) {
        let mut done = Vec::new();
        self.jobs.retain(|j| {
            if j.child.exited() {
                j.child.release();
                done.push((j.child.thread_id, j.name.clone()));
                false
            } else {
                true
            }
        });
        for (tid, name) in done {
            out(DIM, &format!("[{tid}] {name} done"));
        }
    }

    fn prompt(&self) {
        if let Some(c) = entry::console() {
            let path = format!(" {} ", self.cwd);
            c.set_prompt(&[(ACCENT, "daryl@tinyos"), (DIM, &path), (ACCENT, "> ")]);
        }
    }

    fn exec(&mut self, line: &str) -> bool {
        let (line, background) = match line.strip_suffix('&') {
            Some(rest) => (rest.trim(), true),
            None => (line, false),
        };
        let mut it = line.split_whitespace();
        let cmd = it.next().unwrap_or("");
        let args: Vec<&str> = it.collect();
        let a = |i: usize| args.get(i).copied().unwrap_or("");
        let rest = || {
            let p = line.find(char::is_whitespace);
            p.map(|i| line[i..].trim()).unwrap_or("")
        };
        match cmd {
            "" => {}
            "exit" | "logout" => return false,
            "help" => {
                out(FG, "commands:");
                for (c, d) in [
                    ("help", "this list"),
                    ("echo <text>", "print text"),
                    ("clear", "clear the screen"),
                    ("ls [path]", "list a directory"),
                    ("cat <file>", "print a file"),
                    ("write <file> <text>", "write text to a file"),
                    ("append <file> <text>", "append text to a file"),
                    ("touch <file>", "create an empty file"),
                    ("mkdir <dir>", "create a directory"),
                    ("rm [-r] <path>", "remove a file or directory"),
                    ("mv <from> <to>", "move or rename"),
                    ("cp <from> <to>", "copy a file"),
                    ("cd [dir]", "change directory"),
                    ("pwd", "print working directory"),
                    ("fsinfo", "filesystem usage"),
                    ("edit <file>", "edit a file (window)"),
                    ("vi <file>", "edit a file (vi)"),
                    ("run <name> [&]", "run an app (& = background)"),
                    ("jobs", "list background jobs"),
                    ("ps", "list threads and processes"),
                    ("kill <id>", "stop a thread"),
                    ("sysinfo / memstat", "system and memory info"),
                    ("uptime / date", "time since boot / clock"),
                    ("spin [n]", "spawn n busy threads"),
                    ("shutdown / reboot", "sync the disk and power off/restart"),
                    ("about", "about tinyOS"),
                ] {
                    out(FG, &format!("  {c:<22} {d}"));
                }
            }
            "echo" => out(FG, rest()),
            "clear" => {
                if let Some(c) = entry::console() {
                    c.clear();
                }
            }
            "about" => {
                out(ACCENT, "tinyOS - a tiny operating system written in Rust");
                out(FG, "UEFI boot, software-composited GUI, real EL0 userspace,");
                out(FG, "capability ABI, a shell that lives in userspace.");
            }
            "pwd" => out(FG, &self.cwd.clone()),
            "cd" => {
                let target = resolve(&self.cwd, if args.is_empty() { "/" } else { a(0) });
                match fs::stat(&target) {
                    Ok((KIND_DIR, _)) => self.cwd = target,
                    Ok(_) => err(&format!("cd: {target}: not a directory")),
                    Err(st) => err(&format!("cd: {target}: fs error {st}")),
                }
            }
            "ls" => {
                let target = resolve(&self.cwd, if args.is_empty() { "." } else { a(0) });
                match fs::list(&target) {
                    Ok(entries) => {
                        for (name, kind, size) in entries {
                            if kind == KIND_DIR {
                                out(ACCENT, &format!("{:>10}  {name}/", "-"));
                            } else {
                                out(FG, &format!("{size:>10}  {name}"));
                            }
                        }
                    }
                    Err(st) => err(&format!("ls: fs error {st}")),
                }
            }
            "cat" => match fs::read(&resolve(&self.cwd, a(0))) {
                Ok(data) => match core::str::from_utf8(&data) {
                    Ok(text) => {
                        for line in text.lines() {
                            out(FG, line);
                        }
                    }
                    Err(_) => out(DIM, &format!("cat: binary file ({} bytes)", data.len())),
                },
                Err(st) => err(&format!("cat: fs error {st}")),
            },
            "write" | "append" => match rest().split_once(' ') {
                Some((file, text)) => {
                    let p = resolve(&self.cwd, file.trim());
                    let res = if cmd == "append" {
                        let mut cur = fs::read(&p).unwrap_or_default();
                        cur.extend_from_slice(text.as_bytes());
                        fs::write(&p, &cur)
                    } else {
                        fs::write(&p, text.as_bytes())
                    };
                    if let Err(st) = res {
                        err(&format!("{cmd}: fs error {st}"));
                    }
                }
                None => err(&format!("usage: {cmd} <file> <text>")),
            },
            "touch" => {
                let p = resolve(&self.cwd, a(0));
                if fs::stat(&p).is_err() {
                    if let Err(st) = fs::write(&p, &[]) {
                        err(&format!("touch: fs error {st}"));
                    }
                }
            }
            "mkdir" => {
                if let Err(st) = fs::mkdir(&resolve(&self.cwd, a(0))) {
                    err(&format!("mkdir: fs error {st}"));
                }
            }
            "rm" => {
                let (rec, p) = if a(0) == "-r" { (true, a(1)) } else { (false, a(0)) };
                if p.is_empty() {
                    err("usage: rm [-r] <path>");
                } else if let Err(st) = fs::remove(&resolve(&self.cwd, p), rec) {
                    err(&format!("rm: fs error {st}"));
                }
            }
            "mv" => {
                if let Err(st) =
                    fs::rename(&resolve(&self.cwd, a(0)), &resolve(&self.cwd, a(1)))
                {
                    err(&format!("mv: fs error {st}"));
                }
            }
            "cp" => match fs::read(&resolve(&self.cwd, a(0))) {
                Ok(data) => {
                    if let Err(st) = fs::write(&resolve(&self.cwd, a(1)), &data) {
                        err(&format!("cp: fs error {st}"));
                    }
                }
                Err(st) => err(&format!("cp: {}: fs error {st}", a(0))),
            },
            "fsinfo" | "df" => match fs::statfs() {
                Ok((gen, bu, bt, iu, it)) => {
                    out(FG, &format!("tinyfs, generation {gen}"));
                    out(FG, &format!("blocks:  {bu} used / {bt} total"));
                    out(FG, &format!("inodes:  {iu} used / {it} total"));
                }
                Err(st) => err(&format!("fsinfo: error {st}")),
            },
            "edit" | "vi" => {
                if args.is_empty() {
                    err(&format!("usage: {cmd} <file>"));
                } else {
                    // Resolve the path so the app (base "/") finds it.
                    let p = resolve(&self.cwd, a(0));
                    // edit auto-backgrounds (windowed); vi stays foreground
                    // (console surface app) — run() decides from child.windowed.
                    self.run(cmd, &[&p], background);
                }
            }
            "run" => {
                if args.is_empty() {
                    err("usage: run <name> [args...] [&]");
                } else {
                    self.run(a(0), &args[1..], background);
                }
            }
            "jobs" => {
                if self.jobs.is_empty() {
                    out(DIM, "no background jobs");
                } else {
                    for j in &self.jobs {
                        out(FG, &format!("[{}] {}", j.child.thread_id, j.name));
                    }
                }
            }
            "ps" => match proc::ps() {
                Ok((threads, procs)) => {
                    out(DIM, &format!("{:>4}  {:<10} {:>3}  STATE", "ID", "NAME", "CPU"));
                    for t in threads {
                        out(FG, &format!("{:>4}  {:<10} {:>3}  {}", t.id, t.name, t.cpu, t.state));
                    }
                    for p in procs {
                        out(FG, &format!("pid {:>3}  {:<10} thr {}  {} KiB", p.pid, p.name, p.tid, p.mem >> 10));
                    }
                }
                Err(st) => err(&format!("ps: error {st}")),
            },
            "kill" => match a(0).parse::<u32>() {
                Ok(id) => match proc::kill(id) {
                    Ok(()) => out(FG, &format!("kill: signalled {id}")),
                    Err(st) => err(&format!("kill: error {st}")),
                },
                Err(_) => err("usage: kill <id>"),
            },
            "sysinfo" => match proc::sysinfo() {
                Ok(i) => {
                    out(FG, &format!("heap:    {} MiB used / {} MiB total", i.heap_used >> 20, (i.heap_used + i.heap_free) >> 20));
                    out(FG, &format!("frames:  {} MiB used / {} MiB total", (i.pool_total - i.pool_free) >> 20, i.pool_total >> 20));
                    out(FG, &format!("uptime:  {}", fmt_hms(i.uptime_us / 1_000_000)));
                }
                Err(st) => err(&format!("sysinfo: error {st}")),
            },
            "memstat" | "mem" => match proc::sysinfo() {
                Ok(i) => out(FG, &format!("heap: {} KiB used, {} KiB free", i.heap_used >> 10, i.heap_free >> 10)),
                Err(st) => err(&format!("mem: error {st}")),
            },
            "uptime" => match proc::sysinfo() {
                Ok(i) => out(FG, &fmt_hms(i.uptime_us / 1_000_000)),
                Err(st) => err(&format!("uptime: error {st}")),
            },
            "date" => match proc::sysinfo() {
                Ok(i) => {
                    let s = 9 * 3600 + 41 * 60 + i.uptime_us / 1_000_000;
                    out(FG, &format!("Fri Jul 17 {} 2026", fmt_hms(s % 86_400)));
                }
                Err(st) => err(&format!("date: error {st}")),
            },
            "spin" => {
                let n: u32 = a(0).parse().unwrap_or(1).clamp(1, 16);
                match proc::spin(n) {
                    Ok(()) => out(FG, &format!("spawned {n} spin thread(s)")),
                    Err(st) => err(&format!("spin: error {st}")),
                }
            }
            "shutdown" | "poweroff" | "halt" => match proc::shutdown() {
                Ok(()) => {}
                Err(st) => err(&format!("shutdown: error {st}")),
            },
            "reboot" => match proc::reboot() {
                Ok(()) => {}
                Err(st) => err(&format!("reboot: error {st}")),
            },
            "sudo" => err("daryl is not in the sudoers file. This incident will be reported."),
            _ => err(&format!("{cmd}: command not found (try help)")),
        }
        true
    }
}

fn main(env: Env) -> i32 {
    let mut sh = Shell { env, cwd: String::from("/"), jobs: Vec::new() };
    out(DIM, "tinyOS shell - type 'help' to get started");
    loop {
        sh.reap();
        sh.prompt();
        let Some(line) = tinyos_app::read_line() else { break };
        if !sh.exec(line.trim()) {
            break;
        }
    }
    0
}

app!(main);

// The shell: console I/O, broad fs (arbitrary user paths), and process control
// including kill (Ctrl+C, shutdown/reboot). Its grants actually arrive as
// explicit handles from the terminal that spawns it (SYS_PROCESS_SPAWN doesn't
// consult the manifest), so this declaration documents intent + keeps it honest.
tinyos_app::declare_caps!(b"console\nfs:/\nproc.kill");
