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

    // Subtree capabilities (OP_OPEN_DIR): confinement + revocation, driven
    // over a real service. Needs a mounted filesystem; skipped without one.
    if crate::fs::resolve_dir("/", "/").is_ok() {
        use abi::fs::{FS_NOT_FOUND, FS_OK, OP_OPEN_DIR, OP_STAT, R_OPEN_DIR, R_STAT};
        let _ = crate::fs::mkdir("/", "/data");
        let _ = crate::fs::mkdir("/", "/data/jt");
        let _ = crate::fs::mkdir("/", "/data/jt/sub");
        let _ = crate::fs::write("/", "/data/jt/secret", b"s", false);

        let rpc = |svc: &mut crate::fs::service::FsService,
                   end: &alloc::sync::Arc<channel::ChannelEnd>,
                   op: u32,
                   payload: &[u8]| {
            let mut b = op.to_le_bytes().to_vec();
            b.extend_from_slice(payload);
            let _ = end.send(Message { bytes: b, handles: Vec::new() });
            svc.pump();
            end.recv()
        };

        let (app, kern) = channel::create();
        let mut svc = crate::fs::service::FsService::new(
            kern,
            String::from("/data/jt"),
            String::from("/"),
        );
        // Mint a /sub capability; the reply carries the child channel.
        let child = match rpc(&mut svc, &app, OP_OPEN_DIR, b"/sub") {
            Ok(m)
                if m.bytes.get(0..8)
                    == Some(&[R_OPEN_DIR.to_le_bytes(), FS_OK.to_le_bytes()].concat()[..]) =>
            {
                m.handles.into_iter().next().and_then(|h| match h.object {
                    Object::Channel(c) => Some(c),
                    _ => None,
                })
            }
            _ => None,
        };
        check("open_dir mints child channel", child.is_some());
        if let Some(child) = child {
            // The child is confined to /sub: the parent's file is invisible
            // even via .. (which clamps at the child's jail root).
            let stat = |svc: &mut crate::fs::service::FsService,
                        end: &alloc::sync::Arc<channel::ChannelEnd>,
                        p: &[u8]| {
                match rpc(svc, end, OP_STAT, p) {
                    Ok(m) if m.bytes.get(0..4) == Some(&R_STAT.to_le_bytes()[..]) => m
                        .bytes
                        .get(4..8)
                        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                        .unwrap_or(u32::MAX),
                    _ => u32::MAX,
                }
            };
            check(
                "open_dir child is confined",
                stat(&mut svc, &child, b"/secret") == FS_NOT_FOUND
                    && stat(&mut svc, &child, b"../secret") == FS_NOT_FOUND
                    && stat(&mut svc, &app, b"/secret") == FS_OK,
            );
            // Dropping the parent service revokes the child capability.
            drop(svc);
            check(
                "open_dir revoked with parent",
                child.signals() & SIG_PEER_CLOSED != 0,
            );
        }
        let _ = crate::fs::remove("/", "/data/jt", true);
    }

    out
}

static CROSS_TX: spin::Mutex<Option<alloc::sync::Arc<channel::ChannelEnd>>> =
    spin::Mutex::new(None);
