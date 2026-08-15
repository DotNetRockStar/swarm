//! Peer connectivity for SWARM devices.
//!
//! Implemented (Phase 2):
//! - [`identity`] — self-signed device certificate (rcgen), persisted,
//!   SHA-256 fingerprint as peer identity.
//! - [`pin`] — rustls verifiers for fingerprint pinning: one expected pin on
//!   the dialing side, a live allowed-set (swarm roster) on the accepting
//!   side. No CA, no hostname.
//! - [`endpoint`] — quinn QUIC endpoints wired to those verifiers + the
//!   one-request-per-bidi-stream peer framing from `swarm-core::peer`.
//!
//! Planned (Phase 4): reflector client, hole punching, candidate ordering
//! with persisted route promotion, UPnP/NAT-PMP, loopback HTTP<->QUIC proxy
//! (see the recovered references in `docs/reference/`).

pub mod endpoint;
pub mod identity;
pub mod pin;

pub use swarm_core as core;
