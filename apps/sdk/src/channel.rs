//! Channel client: send/recv bytes + handles, and a blocking recv built on
//! wait_many (blocking lives here in the SDK, not the kernel).

use alloc::vec;
use alloc::vec::Vec;

use crate::syscall::*;

pub const INVALID: u32 = 0;

/// A handle to one end of a channel.
#[derive(Clone, Copy)]
pub struct Channel(pub u32);

/// One received message.
pub struct Msg {
    pub bytes: Vec<u8>,
    pub handles: Vec<u32>,
}

impl Channel {
    pub fn create() -> Result<(Channel, Channel), u32> {
        let mut out = [0u32; 2];
        syscall1(SYS_CHANNEL_CREATE, out.as_mut_ptr() as u64).ok()?;
        Ok((Channel(out[0]), Channel(out[1])))
    }

    pub fn send(&self, bytes: &[u8], handles: &[u32]) -> Result<(), u32> {
        syscall6(
            SYS_CHANNEL_SEND,
            self.0 as u64,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            handles.as_ptr() as u64,
            handles.len() as u64,
            0,
        )
        .ok()?;
        Ok(())
    }

    /// Non-blocking receive. Returns Err(ST_SHOULD_WAIT) if empty.
    pub fn try_recv(&self) -> Result<Msg, u32> {
        let mut lens = [0u32; 2];
        // First call reports required sizes without consuming.
        let probe = syscall6(
            SYS_CHANNEL_RECV,
            self.0 as u64,
            0,
            0,
            0,
            0,
            lens.as_mut_ptr() as u64,
        );
        if probe.status != ST_BUFFER_TOO_SMALL && probe.status != ST_OK {
            return Err(probe.status);
        }
        let mut bytes = vec![0u8; lens[0] as usize];
        let mut handles = vec![0u32; lens[1] as usize];
        syscall6(
            SYS_CHANNEL_RECV,
            self.0 as u64,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            handles.as_mut_ptr() as u64,
            handles.len() as u64,
            lens.as_mut_ptr() as u64,
        )
        .ok()?;
        Ok(Msg { bytes, handles })
    }

    /// Block until a message arrives (or the peer closes / deadline passes).
    pub fn recv(&self) -> Result<Msg, u32> {
        loop {
            match self.try_recv() {
                Err(ST_SHOULD_WAIT) => {
                    crate::wait::wait_one(self.0, SIG_READABLE | SIG_PEER_CLOSED, u64::MAX)?;
                }
                other => return other,
            }
        }
    }

    pub fn close(&self) {
        let _ = syscall1(SYS_HANDLE_CLOSE, self.0 as u64);
    }
}
