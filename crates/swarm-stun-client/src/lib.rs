//! Device-side STUN client.
//!
//! - [`client`] — REST calls: register a device with a join code, join
//!   additional swarms, fetch a swarm's device roster.
//! - [`token_store`] — encrypted-at-rest access-token storage (OS keychain,
//!   with a permission-restricted file fallback).
//! - [`machine_id`] — stable per-install identity submitted at registration.
//!
//! Planned (Phase 4): a persistent WSS session (presence + hole-punch
//! signal exchange), matching the shape of the recovered `mux_client.py`
//! (capped-backoff reconnect, per-transfer channel demux).

pub mod client;
pub mod machine_id;
pub mod token_store;

pub use client::{StunClient, StunClientError};
pub use token_store::{TokenStore, TokenStoreError};

pub use swarm_core as core;
