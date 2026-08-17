//! SWARM STUN server library — exposed so integration tests can compose the
//! router in-process. The binary entrypoint is `main.rs`.

pub mod authn;
pub mod config;
pub mod db;
pub mod email;
pub mod error;
pub mod hub;
pub mod reflector;
pub mod routes;
pub mod security;
pub mod state;
