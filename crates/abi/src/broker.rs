//! Service broker protocol v0. A client sends OP_CONNECT (bytes = [OP_CONNECT])
//! on a broker channel; the server replies R_CONNECTED{status:u32}, and on
//! BROKER_OK the new connection's client end rides as the reply's single moved
//! handle. Identical whether the server is in-kernel (SP0) or a userspace fsd
//! later. See docs/superpowers/specs/2026-07-19-service-brokers-design.md.

pub const OP_CONNECT: u32 = 1;
pub const R_CONNECTED: u32 = 2;

pub const BROKER_OK: u32 = 0;
pub const BROKER_NOMEM: u32 = 1;
