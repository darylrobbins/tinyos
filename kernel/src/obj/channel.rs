//! Bidirectional message channels: the IPC spine. Each end owns its RX
//! queue; sending pushes into the peer's queue. Messages carry bytes and
//! moved handles. Queues are bounded (SHOULD_WAIT on full).

use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use spin::Mutex;

use super::handle::Handle;
use super::syscall::{ST_PEER_CLOSED, ST_SHOULD_WAIT};

pub const MAX_MSGS: usize = 64;
pub const MAX_BYTES: usize = 64 * 1024;
/// Per-message caps (also the ABI's documented limits).
pub const MAX_MSG_BYTES: usize = 32 * 1024;
pub const MAX_MSG_HANDLES: usize = 8;

pub struct Message {
    pub bytes: Vec<u8>,
    pub handles: Vec<Handle>,
}

struct Rx {
    queue: VecDeque<Message>,
    queued_bytes: usize,
}

pub struct ChannelEnd {
    rx: Mutex<Rx>,
    peer: Mutex<Weak<ChannelEnd>>,
}

pub fn create() -> (Arc<ChannelEnd>, Arc<ChannelEnd>) {
    let mk = || {
        Arc::new(ChannelEnd {
            rx: Mutex::new(Rx { queue: VecDeque::new(), queued_bytes: 0 }),
            peer: Mutex::new(Weak::new()),
        })
    };
    let (a, b) = (mk(), mk());
    *a.peer.lock() = Arc::downgrade(&b);
    *b.peer.lock() = Arc::downgrade(&a);
    (a, b)
}

impl ChannelEnd {
    /// Deliver `msg` to the peer's RX queue.
    pub fn send(&self, msg: Message) -> Result<(), u32> {
        let peer = self.peer.lock().upgrade().ok_or(ST_PEER_CLOSED)?;
        {
            let mut rx = peer.rx.lock();
            if rx.queue.len() >= MAX_MSGS || rx.queued_bytes + msg.bytes.len() > MAX_BYTES {
                return Err(ST_SHOULD_WAIT);
            }
            rx.queued_bytes += msg.bytes.len();
            rx.queue.push_back(msg);
        }
        super::wake_objects();
        Ok(())
    }

    /// Take the next queued message. Queued messages remain readable after
    /// the peer closes; only an empty queue reports PEER_CLOSED.
    pub fn recv(&self) -> Result<Message, u32> {
        let mut rx = self.rx.lock();
        match rx.queue.pop_front() {
            Some(m) => {
                rx.queued_bytes -= m.bytes.len();
                Ok(m)
            }
            None => {
                if self.peer.lock().upgrade().is_none() {
                    Err(ST_PEER_CLOSED)
                } else {
                    Err(ST_SHOULD_WAIT)
                }
            }
        }
    }

    /// (bytes, handles) of the next message without consuming it.
    pub fn peek(&self) -> Option<(usize, usize)> {
        let rx = self.rx.lock();
        rx.queue.front().map(|m| (m.bytes.len(), m.handles.len()))
    }

    pub fn signals(&self) -> u32 {
        let mut s = 0;
        let readable = !self.rx.lock().queue.is_empty();
        if readable {
            s |= super::SIG_READABLE;
        }
        match self.peer.lock().upgrade() {
            Some(peer) => {
                let rx = peer.rx.lock();
                if rx.queue.len() < MAX_MSGS && rx.queued_bytes < MAX_BYTES {
                    s |= super::SIG_WRITABLE;
                }
            }
            None => s |= super::SIG_PEER_CLOSED,
        }
        s
    }
}

impl Drop for ChannelEnd {
    fn drop(&mut self) {
        // The peer (if any) now observes PEER_CLOSED; wake waiters so they
        // re-evaluate. Runs only in thread context (handle close / teardown).
        super::wake_objects();
    }
}
