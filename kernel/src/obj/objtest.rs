//! Kernel-side unit tests for the object model (terminal `objtest`).
//! In lieu of a hosted test harness, each case returns a PASS/FAIL line.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::channel::{self, MAX_MSGS, Message};
use super::handle::{Handle, HandleTable, RIGHT_DUP, RIGHT_READ, RIGHT_WRITE, RIGHTS_ALL};
use super::memobj::MemObj;
use super::syscall::{ST_OK, ST_PEER_CLOSED, ST_SHOULD_WAIT};
use super::{Object, SIG_PEER_CLOSED, SIG_READABLE, wait_many};

pub fn run() -> Vec<String> {
    let mut out = Vec::new();
    let mut check = |name: &str, ok: bool| out.push(format!("{} {name}", if ok { "PASS" } else { "FAIL" }));

    // Channel round-trip.
    let (a, b) = channel::create();
    let sent = a
        .send(Message { bytes: alloc::vec![1, 2, 3], handles: Vec::new() })
        .is_ok();
    let got = b.recv();
    check(
        "channel round-trip",
        sent && matches!(&got, Ok(m) if m.bytes == [1, 2, 3]),
    );
    check("empty recv is SHOULD_WAIT", matches!(b.recv(), Err(e) if e == ST_SHOULD_WAIT));

    // Bounded queue.
    let mut full = None;
    for i in 0..=MAX_MSGS {
        if let Err(e) = a.send(Message { bytes: alloc::vec![0], handles: Vec::new() }) {
            full = Some((i, e));
            break;
        }
    }
    check("bounded queue", full == Some((MAX_MSGS, ST_SHOULD_WAIT)));

    // Peer close: drain queued, then PEER_CLOSED; send fails immediately.
    drop(a);
    let mut drained = 0;
    while b.recv().is_ok() {
        drained += 1;
    }
    check(
        "close: drain then PEER_CLOSED",
        drained == MAX_MSGS
            && matches!(b.recv(), Err(e) if e == ST_PEER_CLOSED)
            && b.send(Message { bytes: Vec::new(), handles: Vec::new() }) == Err(ST_PEER_CLOSED)
            && b.signals() & SIG_PEER_CLOSED != 0,
    );

    // Handle table + rights narrowing.
    let (c, _d) = channel::create();
    let mut table = HandleTable::new();
    let hv = table.insert(Handle::new(Object::Channel(c), RIGHTS_ALL)).unwrap();
    let narrowed = table.dup(hv, RIGHT_READ | RIGHT_DUP).unwrap();
    let widened_stays_narrow = table.dup(narrowed, RIGHTS_ALL).unwrap();
    check(
        "dup narrows, never widens",
        table.get(narrowed).unwrap().rights == RIGHT_READ | RIGHT_DUP
            && table.get(widened_stays_narrow).unwrap().rights == RIGHT_READ | RIGHT_DUP,
    );
    let no_dup = table.dup(hv, RIGHT_WRITE).unwrap();
    check("dup without DUP right denied", table.dup(no_dup, RIGHTS_ALL).is_err());
    check(
        "close removes",
        table.take(hv).is_ok() && table.get(hv).is_err(),
    );

    // MemObj: kernel-visible backing.
    let m = MemObj::create(10_000).unwrap();
    unsafe { m.bytes()[9_999] = 0xAB };
    check(
        "memobj create/write/read",
        m.size() == 10_000 && unsafe { m.bytes()[9_999] } == 0xAB && m.pa() % 4096 == 0,
    );

    // Cross-thread wait: a worker sends after 30ms; we wait on READABLE.
    let (tx, rx) = channel::create();
    CROSS_TX.lock().replace(tx);
    crate::sched::spawn(
        String::from("objtest-tx"),
        crate::sched::thread::Class::Normal,
        0xF,
        || {
            crate::sched::sleep_us(30_000);
            if let Some(tx) = CROSS_TX.lock().take() {
                let _ = tx.send(Message { bytes: alloc::vec![9], handles: Vec::new() });
            }
        },
    );
    let t0 = crate::arch::timer::uptime_us();
    let mut sets = [(Object::Channel(rx.clone()), SIG_READABLE, 0u32)];
    let st = wait_many(&mut sets, t0 + 2_000_000);
    let waited = crate::arch::timer::uptime_us() - t0;
    check(
        "cross-thread wait wakes",
        st == ST_OK && sets[0].2 & SIG_READABLE != 0 && waited >= 25_000 && rx.recv().is_ok(),
    );

    // FS jail: no app-supplied path may resolve outside the jail root.
    let jp = crate::fs::service::jailed_path;
    check(
        "jail: absolute path re-roots",
        jp("/data/app", "/", "/apps/evil") == Ok(String::from("/data/app/apps/evil")),
    );
    check(
        "jail: .. clamps at jail root",
        jp("/data/app", "/", "../../../apps/evil") == Ok(String::from("/data/app/apps/evil"))
            && jp("/data/app", "/", "..") == Ok(String::from("/data/app"))
            && jp("/data/app", "/sub", "../../..") == Ok(String::from("/data/app")),
    );
    check(
        "jail: '/' jail stays unconfined",
        jp("/", "/home", "../x") == Ok(String::from("/x")),
    );

    out
}

static CROSS_TX: spin::Mutex<Option<alloc::sync::Arc<channel::ChannelEnd>>> =
    spin::Mutex::new(None);
