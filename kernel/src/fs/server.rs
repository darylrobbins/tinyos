//! Standing FS server: a broker channel that mints a fresh, isolated FsService
//! connection per OP_CONNECT, plus the pool of live connections it pumps. The
//! same broker protocol whether served here or by a userspace fsd later.
//! See docs/superpowers/specs/2026-07-19-service-brokers-design.md.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use abi::broker::{BROKER_OK, OP_CONNECT, R_CONNECTED};

use crate::fs::service::FsService;
use crate::obj::channel::{self, ChannelEnd, Message};
use crate::obj::handle::{Handle, RIGHTS_ALL};
use crate::obj::Object;

pub struct FsServer {
    broker: Arc<ChannelEnd>,
    conns: Vec<FsService>,
}

impl FsServer {
    pub fn new(broker: Arc<ChannelEnd>) -> Self {
        Self { broker, conns: Vec::new() }
    }

    /// Mint one fresh, full-root connection; pool the server end and return the
    /// CLIENT end as a transferable handle. The single connection-creation path
    /// shared by direct in-kernel callers and the broker below.
    pub fn mint(&mut self) -> Handle {
        let (client, server) = channel::create();
        self.conns
            .push(FsService::new(server, String::from("/"), String::from("/")));
        Handle::new(Object::Channel(client), RIGHTS_ALL)
    }

    /// Serve queued OP_CONNECTs, then pump every live connection, reaping any
    /// whose client end has closed.
    pub fn pump(&mut self) {
        while let Ok(msg) = self.broker.recv() {
            let op = msg.bytes.get(0..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            if op == Some(OP_CONNECT) {
                let h = self.mint();
                let mut reply = R_CONNECTED.to_le_bytes().to_vec();
                reply.extend_from_slice(&BROKER_OK.to_le_bytes());
                let _ = self.broker.send(Message { bytes: reply, handles: vec![h] });
            }
        }
        self.conns.retain_mut(|c| {
            c.pump();
            c.is_open()
        });
    }
}
