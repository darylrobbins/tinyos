//! Broker client: request a fresh, private service connection from a broker
//! channel the spawner granted us. Blocks for the reply.

use crate::channel::Channel;
use abi::broker::{BROKER_OK, OP_CONNECT, R_CONNECTED};

fn le(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap()))
}

/// Malformed / handle-less broker reply (distinct from any BROKER_* status).
pub const ERR_BROKER_REPLY: u32 = u32::MAX;

/// Ask `broker` for a new connection. Returns the connection's client end.
pub fn connect(broker: Channel) -> Result<Channel, u32> {
    broker.send(&OP_CONNECT.to_le_bytes(), &[])?;
    let msg = broker.recv()?;
    if le(&msg.bytes, 0) != Some(R_CONNECTED) {
        return Err(ERR_BROKER_REPLY);
    }
    match le(&msg.bytes, 4) {
        Some(BROKER_OK) => msg.handles.first().copied().map(Channel).ok_or(ERR_BROKER_REPLY),
        Some(e) => Err(e),
        None => Err(ERR_BROKER_REPLY),
    }
}
