//! heartbeatd — a minimal demo system service. Publishes "heartbeat" to the
//! Nexus (proving spawn + jail + publish), then stays alive. Killing it proves
//! svcd's restart. Not a real service — it exists to exercise the supervisor.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;

use tinyos_app::channel::Channel;
use tinyos_app::syscall::{syscall2, SYS_LOG};
use tinyos_app::wait::sleep_us;
use tinyos_app::{app, entry::Env, nexus};

fn log(s: &str) {
    let b = s.as_bytes();
    syscall2(SYS_LOG, b.as_ptr() as u64, b.len() as u64);
}

fn main(env: Env) -> i32 {
    // Create an endpoint others could connect to, and publish its far end.
    let (keep, endpoint) = match Channel::create() {
        Ok(p) => p,
        Err(e) => {
            log(&format!("heartbeatd: channel create failed ({e})"));
            return 1;
        }
    };
    match nexus::publish(env.nexus, "heartbeat", endpoint.0) {
        Ok(()) => log("heartbeatd: published heartbeat"),
        Err(e) => {
            log(&format!("heartbeatd: publish failed ({e})"));
            return 1;
        }
    }
    // Stay alive (a real service would serve `keep`). Hold `keep` so the
    // endpoint stays live.
    let _ = keep;
    loop {
        sleep_us(60_000_000);
    }
}

app!(main);
