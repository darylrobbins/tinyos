//! waiterd — a minimal demo service that depends on "heartbeat". It looks the
//! name up on the Nexus (which BLOCKS until heartbeatd publishes), then logs
//! success and exits. Proves readiness ordering: waiterd cannot proceed until
//! its dependency is ready, with no pid/socket rendezvous.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;

use tinyos_app::syscall::{syscall1, syscall2, SYS_HANDLE_CLOSE, SYS_LOG};
use tinyos_app::{app, entry::Env, nexus};

fn log(s: &str) {
    let b = s.as_bytes();
    syscall2(SYS_LOG, b.as_ptr() as u64, b.len() as u64);
}

fn main(env: Env) -> i32 {
    log("waiterd: waiting for heartbeat");
    match nexus::lookup(env.nexus, "heartbeat") {
        Ok(h) => {
            log("waiterd: heartbeat ready");
            let _ = syscall1(SYS_HANDLE_CLOSE, h as u64);
            0
        }
        Err(e) => {
            log(&format!("waiterd: lookup failed ({e})"));
            1
        }
    }
}

app!(main);
