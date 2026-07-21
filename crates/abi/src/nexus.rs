//! Nexus protocol (v0): a named service registry with readiness.
//!
//! A provider `OP_PUBLISH`es its service endpoint (a moved channel handle)
//! under a name; a consumer `OP_LOOKUP`s by name and BLOCKS until a matching
//! publish arrives, then receives a handle to that endpoint. Readiness is thus
//! "the producer published", with no pid/socket rendezvous.
//!
//! svcd hosts the Nexus in v1: each service receives a client channel as
//! bootstrap grant `TAG_NEXUS`. Requests/replies ride that per-connection
//! channel (not a shared broker queue).
//!
//! Wire format (all integers LE):
//! - `OP_PUBLISH`: `[op:u32][namelen:u32][name:utf8]` + one moved handle (the
//!   endpoint). Reply `R_STATUS`: `[R_STATUS][status]`.
//! - `OP_LOOKUP`:  `[op:u32][namelen:u32][name:utf8]`. Reply `R_LOOKUP`:
//!   `[R_LOOKUP][status]` + (on `NX_OK`) one moved handle (a dup of the
//!   endpoint). The reply is withheld until a producer publishes `name`.
//! - `OP_LIST`:    `[op:u32]`. Reply `R_LIST`: `[R_LIST][count:u32]` then per
//!   entry `[namelen:u32][name:utf8]`.

pub const OP_PUBLISH: u32 = 1;
pub const OP_LOOKUP: u32 = 2;
pub const OP_LIST: u32 = 3;

pub const R_STATUS: u32 = 64;
pub const R_LOOKUP: u32 = 65;
pub const R_LIST: u32 = 66;

pub const NX_OK: u32 = 0;
pub const NX_LIMIT: u32 = 1;
pub const NX_INVALID: u32 = 2;

/// Max service-name length (bytes).
pub const MAX_NAME: usize = 64;
