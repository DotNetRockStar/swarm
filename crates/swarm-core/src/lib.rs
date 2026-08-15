//! SWARM shared contracts — the single source of truth for every message that
//! crosses a process boundary in the SWARM suite.
//!
//! Three protocol planes are defined here:
//! - [`rest`] — device ↔ STUN REST contracts (registration, swarms, devices).
//! - [`signal`] — device ↔ STUN signaling messages over WSS (presence,
//!   hole-punch candidate exchange).
//! - [`peer`] — device ↔ device requests carried over hole-punched QUIC
//!   (catalog sync, artwork, media bytes).
//!
//! Plus two identity primitives shared by every component:
//! - [`fingerprint`] — `sample-fp-v1` content identity, byte-compatible with
//!   Batocera.Drone's implementation so the same file yields the same
//!   fingerprint everywhere.
//! - [`entry_key`] — path-derived catalog keys, validated before any
//!   filesystem access.
//!
//! This crate performs no I/O beyond reading files handed to the fingerprint
//! helpers; it must stay dependency-light so the Tauri app, the STUN server,
//! and test harnesses can all share it.

pub mod capability;
pub mod entry_key;
pub mod fingerprint;
pub mod peer;
pub mod rest;
pub mod signal;

/// Wire-format version carried in `hello` and rejected on mismatch, so an old
/// device gets a clear "please update" instead of undefined behavior.
pub const PROTOCOL_VERSION: u32 = 1;
