//! sh — a userspace shell, and the first real user of SYS_PROCESS_SPAWN
//! from EL0. Files over the fs protocol, ps/kill/mem over the proc
//! protocol, `run <name>` by staging /apps/<name> into a memobj and
//! spawning it with capability grants (a dup of this shell's own console
//! and fs channels, so children share the terminal and filesystem —
//! stdin included, since the shell stays out of the console queue while
//! it waits on the child's EXITED signal).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use abi::bootstrap::{TAG_CONSOLE, TAG_FS, TAG_PROC, TAG_SHELL};
use abi::console::INPUT_MODE_LINES;
use abi::fs::KIND_DIR;
use tinyos_app::syscall::{syscall2, SYS_HANDLE_DUP, RIGHTS_ALL};
use tinyos_app::{app, entry, fs, print, println, proc, process, read_line, Env};

/// Join + normalize: absolute paths pass through; relative resolve against
/// `cwd`; "." and ".." handled textually.
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
    let mut out = String::from("/");
    out.push_str(&parts.join("/"));
    out
}

fn run(env: &Env, cwd: &str, name: &str, args: &[&str]) {
    let path = format!("/apps/{name}");
    let elf = match fs::read(&path) {
        Ok(e) => e,
        Err(st) => {
            println!("run: {name}: fs error {st}");
            return;
        }
    };
    // Children inherit our capabilities (dup'd, so we keep ours): console
    // (shared — safe because we don't read it while waiting on the child),
    // fs, the window server, and process control. This lets windowed apps
    // (edit) and surface apps (vi, top) run from the shell.
    let mut grants: Vec<(u32, u32)> = Vec::new();
    for (tag, ch) in [
        (TAG_CONSOLE, env.console.0),
        (TAG_FS, env.fs.0),
        (TAG_SHELL, env.shell.0),
        (TAG_PROC, env.proc.0),
    ] {
        if ch != 0 {
            if let Ok(h) = syscall2(SYS_HANDLE_DUP, ch as u64, RIGHTS_ALL as u64).ok() {
                grants.push((tag, h as u32));
            }
        }
    }
    let _ = cwd; // children see paths relative to the service's base
    match process::spawn(&elf, args, &grants) {
        Ok(child) => {
            let tid = child.thread_id;
            child.wait();
            // A surface/raw-key child may have left the terminal in KEYS
            // mode; restore line editing for our prompt (surface apps also
            // reset on close, this covers the rest).
            if let Some(con) = entry::console() {
                con.set_input_mode(INPUT_MODE_LINES);
            }
            println!("[{tid}] done");
        }
        Err(st) => println!("run: spawn failed (status {st})"),
    }
}

fn main(env: Env) -> i32 {
    println!("tinyOS sh — userspace shell. 'help' for commands, 'exit' to leave.");
    let mut cwd = String::from("/");
    loop {
        print!("sh {cwd} % ");
        let Some(line) = read_line() else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let cmd = it.next().unwrap_or("");
        let args: Vec<&str> = it.collect();
        let arg = |i: usize| args.get(i).copied().unwrap_or("");
        match cmd {
            "exit" => break,
            "help" => {
                println!("files: ls cat write mkdir rm [-r] mv cp touch cd pwd");
                println!("procs: ps kill <id> mem run <name> [args]");
                println!("misc:  help exit");
            }
            "pwd" => println!("{cwd}"),
            "cd" => {
                let target = resolve(&cwd, if args.is_empty() { "/" } else { arg(0) });
                match fs::stat(&target) {
                    Ok((KIND_DIR, _)) => cwd = target,
                    Ok(_) => println!("cd: {target}: not a directory"),
                    Err(st) => println!("cd: {target}: fs error {st}"),
                }
            }
            "ls" => {
                let target = resolve(&cwd, if args.is_empty() { "." } else { arg(0) });
                match fs::list(&target) {
                    Ok(entries) => {
                        for (name, kind, size) in entries {
                            if kind == KIND_DIR {
                                println!("{:>10}  {name}/", "-");
                            } else {
                                println!("{size:>10}  {name}");
                            }
                        }
                    }
                    Err(st) => println!("ls: fs error {st}"),
                }
            }
            "cat" => match fs::read(&resolve(&cwd, arg(0))) {
                Ok(data) => match core::str::from_utf8(&data) {
                    Ok(text) => print!("{text}"),
                    Err(_) => println!("cat: binary file ({} bytes)", data.len()),
                },
                Err(st) => println!("cat: fs error {st}"),
            },
            "write" => {
                let text = args[1..].join(" ");
                if let Err(st) = fs::write(&resolve(&cwd, arg(0)), text.as_bytes()) {
                    println!("write: fs error {st}");
                }
            }
            "touch" => {
                let p = resolve(&cwd, arg(0));
                if fs::stat(&p).is_err() {
                    if let Err(st) = fs::write(&p, &[]) {
                        println!("touch: fs error {st}");
                    }
                }
            }
            "mkdir" => {
                if let Err(st) = fs::mkdir(&resolve(&cwd, arg(0))) {
                    println!("mkdir: fs error {st}");
                }
            }
            "rm" => {
                let (rec, p) = if arg(0) == "-r" { (true, arg(1)) } else { (false, arg(0)) };
                if let Err(st) = fs::remove(&resolve(&cwd, p), rec) {
                    println!("rm: fs error {st}");
                }
            }
            "mv" => {
                if let Err(st) = fs::rename(&resolve(&cwd, arg(0)), &resolve(&cwd, arg(1))) {
                    println!("mv: fs error {st}");
                }
            }
            "cp" => match fs::read(&resolve(&cwd, arg(0))) {
                Ok(data) => {
                    if let Err(st) = fs::write(&resolve(&cwd, arg(1)), &data) {
                        println!("cp: fs error {st}");
                    }
                }
                Err(st) => println!("cp: fs error {st}"),
            },
            "ps" => match proc::ps() {
                Ok((threads, procs)) => {
                    println!("{:>4}  {:<10} {:>3}  STATE", "ID", "NAME", "CPU");
                    for t in threads {
                        println!("{:>4}  {:<10} {:>3}  {}", t.id, t.name, t.cpu, t.state);
                    }
                    for p in procs {
                        println!("pid {:>3}  {:<10} thread {}  {} KiB", p.pid, p.name, p.tid, p.mem >> 10);
                    }
                }
                Err(st) => println!("ps: error {st}"),
            },
            "kill" => match arg(0).parse::<u32>() {
                Ok(id) => match proc::kill(id) {
                    Ok(()) => println!("kill: signalled {id}"),
                    Err(st) => println!("kill: error {st}"),
                },
                Err(_) => println!("usage: kill <id>"),
            },
            "mem" => match proc::sysinfo() {
                Ok(i) => println!(
                    "heap {}/{} MiB, pool {}/{} MiB",
                    i.heap_used >> 20,
                    (i.heap_used + i.heap_free) >> 20,
                    (i.pool_total - i.pool_free) >> 20,
                    i.pool_total >> 20
                ),
                Err(st) => println!("mem: error {st}"),
            },
            "run" => {
                if args.is_empty() {
                    println!("usage: run <name> [args...]");
                } else {
                    run(&env, &cwd, arg(0), &args[1..]);
                }
            }
            _ => println!("sh: {cmd}: not found (try help)"),
        }
    }
    0
}

app!(main);
