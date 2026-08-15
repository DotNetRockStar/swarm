//! Peer connectivity for SWARM devices (Phase 2+).
//!
//! Planned modules, porting Batocera.Drone patterns to Rust:
//! - `identity` — self-signed device cert generation (rcgen), key storage,
//!   SHA-256 fingerprinting (port of `drone_tls.py`'s DroneCertificateManager
//!   without the openssl subprocess).
//! - `pin` — rustls certificate verifier that accepts exactly one pinned
//!   fingerprint per connection (no CA, no hostname check).
//! - `reflector` — UDP reflexive-address client (`b"bind"` -> JSON, the
//!   recovered `holepunch.py` contract).
//! - `punch` — simultaneous UDP hole punching with mutual confirmation over
//!   signaling.
//! - `candidates` — candidate gathering/ordering + persisted last-successful
//!   route promotion ("changes preference, never the trusted set" — port of
//!   `peer_connectivity.py`'s `_peer_address_candidates`).
//! - `upnp` — UPnP/NAT-PMP port mapping for the server role.
//! - `endpoint` — quinn QUIC endpoint over the punched socket.
//! - `proxy` — loopback HTTP/1.1 <-> QUIC-stream bridge used by media players.

pub use swarm_core as core;
