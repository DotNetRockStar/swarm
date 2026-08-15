//! SWARM STUN server (Phase 1).
//!
//! Planned composition (single tokio process):
//! - Axum REST API under `/api/v1` (utoipa-derived OpenAPI, Swagger UI at
//!   `/api/docs`), SQLite via sqlx (WAL).
//! - Server-rendered web UI (Askama) for accounts/swarms/codes/devices.
//! - WSS signaling hub at `/api/v1/ws` (presence + signal routing, enforced
//!   shared-swarm membership).
//! - UDP reflector tasks on 443/3478 (`b"bind"` -> `{"ip","port"}`).
//!
//! Deployed behind Caddy (TLS on 443) via `deploy/docker-compose.yml`.

fn main() {
    println!(
        "swarm-stun-server {} (protocol v{}) — Phase 1 implementation pending",
        env!("CARGO_PKG_VERSION"),
        swarm_core::PROTOCOL_VERSION
    );
}
