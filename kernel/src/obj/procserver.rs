//! Standing PROC server: mirror of FsServer for the process-control protocol.
//! Mints a fresh ProcService connection per OP_CONNECT. SP0 mints every
//! connection with can_kill=true (same authority as today); per-connection
//! privilege is a later policy layer.

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use abi::broker::{BROKER_OK, OP_CONNECT, R_CONNECTED};

use crate::obj::channel::{self, ChannelEnd, Message};
use crate::obj::handle::{Handle, RIGHTS_ALL};
use crate::obj::procsrv::ProcService;
use crate::obj::Object;

pub struct ProcServer {
    broker: Arc<ChannelEnd>,
    conns: Vec<ProcService>,
}

impl ProcServer {
    pub fn new(broker: Arc<ChannelEnd>) -> Self {
        Self { broker, conns: Vec::new() }
    }

    pub fn mint(&mut self) -> Handle {
        let (client, server) = channel::create();
        self.conns.push(ProcService::new(server, true));
        Handle::new(Object::Channel(client), RIGHTS_ALL)
    }

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
