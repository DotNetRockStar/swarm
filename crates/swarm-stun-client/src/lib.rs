//! Device-side STUN client.
//!
//! - [`client`] — REST calls: register a device with a join code, join
//!   additional swarms, fetch a swarm's device roster.
//! - [`signaling`] — the persistent WSS session (presence + hole-punch
//!   signal exchange); see [`signaling::SignalingClient`].
//! - [`token_store`] — encrypted-at-rest access-token storage (OS keychain,
//!   with a permission-restricted file fallback).
//! - [`machine_id`] — stable per-install identity submitted at registration.
//!
//! Planned (Phase 4 remainder): the actual hole-punch mechanics (reflector
//! client, `PUNCH_MAGIC` exchange, UPnP) on top of `signaling`'s candidate
//! relay, and capped-backoff reconnect matching the recovered
//! `mux_client.py`'s shape.

pub mod client;
pub mod machine_id;
pub mod signaling;
pub mod token_store;

pub use client::{StunClient, StunClientError};
pub use signaling::{SignalingClient, SignalingError};
pub use token_store::{TokenStore, TokenStoreError};

pub use swarm_core as core;
