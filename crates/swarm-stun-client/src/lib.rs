//! Device-side STUN client (Phase 1+).
//!
//! Planned modules:
//! - `register` — join-code redemption + re-registration after 401 (the
//!   recovered Overmind two-token recovery flow).
//! - `session` — persistent WSS connection with capped-backoff reconnect
//!   (port of the recovered `mux_client.py` runner shape), presence handling,
//!   signal dispatch to `swarm-p2p`'s punch negotiator.
//! - `machine_id` — stable machine identity (persisted NIC-MAC pattern from
//!   `device_identity.py`, with macOS/Windows branches).
//!
//! Also the backbone of the headless test binaries used by the integration
//! harness (Phase 1 exit criteria).

pub use swarm_core as core;
