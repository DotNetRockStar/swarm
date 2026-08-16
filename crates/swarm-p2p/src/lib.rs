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
//! - [`local_addr`] — LAN-facing address self-detection (zero-packet UDP
//!   route-table probe), for a server to self-report where it can be dialed.
//! - [`reflector`] — reflexive (NAT-mapped) address discovery client, the
//!   device half of `apps/stun-server`'s UDP reflector.
//! - [`punch`] — the simultaneous `PUNCH_MAGIC` UDP hole-punch handshake.
//!
//! Planned (Phase 4 remainder): the orchestration that ties `punch`,
//! `reflector`, `local_addr`, and `swarm-stun-client::signaling` together
//! (gather candidates, exchange `Offer`/`Answer`, punch, wait for mutual
//! `Punched` confirmation, *then* dial `endpoint::connect`) — deliberately
//! not in this crate itself, see `punch`'s doc comment — plus candidate
//! ordering with persisted route promotion and UPnP/NAT-PMP. The loopback
//! HTTP<->QUIC proxy landed on the Kotlin client's `:core/proxy`; see the
//! recovered references in `docs/reference/` for background on the
//! original protocol this one is modeled on.

pub mod endpoint;
pub mod identity;
pub mod local_addr;
pub mod pin;
pub mod punch;
pub mod reflector;

pub use swarm_core as core;
