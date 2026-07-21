//! Nexus client (abi::nexus v0): publish a service endpoint under a name, or
//! look one up by name (blocking until a provider publishes). Both block for
//! the reply on the `TAG_NEXUS` channel svcd granted us.

use alloc::vec::Vec;

use abi::nexus::*;

use crate::channel::Channel;

fn le(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|c| u32::from_le_bytes(c.try_into().unwrap()))
}

fn name_req(op: u32, name: &str) -> Vec<u8> {
    let mut r = op.to_le_bytes().to_vec();
    r.extend_from_slice(&(name.len() as u32).to_le_bytes());
    r.extend_from_slice(name.as_bytes());
    r
}

/// Publish `endpoint` under `name`. The handle MOVES to the Nexus. Blocks for
/// the ack.
pub fn publish(nexus: Channel, name: &str, endpoint: u32) -> Result<(), u32> {
    nexus.send(&name_req(OP_PUBLISH, name), &[endpoint]).map_err(|_| NX_INVALID)?;
    let m = nexus.recv().map_err(|_| NX_INVALID)?;
    match (le(&m.bytes, 0), le(&m.bytes, 4)) {
        (Some(R_STATUS), Some(NX_OK)) => Ok(()),
        (Some(R_STATUS), Some(st)) => Err(st),
        _ => Err(NX_INVALID),
    }
}

/// Look up `name`. Blocks until a provider publishes it. Returns a handle to
/// the endpoint (a dup owned by this process).
pub fn lookup(nexus: Channel, name: &str) -> Result<u32, u32> {
    nexus.send(&name_req(OP_LOOKUP, name), &[]).map_err(|_| NX_INVALID)?;
    let m = nexus.recv().map_err(|_| NX_INVALID)?;
    match (le(&m.bytes, 0), le(&m.bytes, 4)) {
        (Some(R_LOOKUP), Some(NX_OK)) => m.handles.first().copied().ok_or(NX_INVALID),
        (Some(R_LOOKUP), Some(st)) => Err(st),
        _ => Err(NX_INVALID),
    }
}
