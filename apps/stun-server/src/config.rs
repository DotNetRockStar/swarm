//! Environment-driven configuration. Every knob has a sane default so
//! `cargo run` works with zero setup; deployment overrides via env.

use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_path: String,
    pub http_bind: SocketAddr,
    /// UDP ports for the reflector; empty disables it (tests).
    pub reflector_ports: Vec<u16>,
    /// External base URL, used in verification/reset links.
    pub public_url: String,
    /// Session lifetime; sliding — extended on use.
    pub session_ttl_secs: i64,
    /// Device access tokens don't expire on a timer; this is the join-code TTL.
    pub join_code_ttl_secs: i64,
}

impl Config {
    pub fn from_env() -> Self {
        let reflector_ports = std::env::var("SWARM_REFLECTOR_PORTS")
            .unwrap_or_else(|_| "9443,3478".into())
            .split(',')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        Self {
            database_path: std::env::var("SWARM_DATABASE_PATH").unwrap_or_else(|_| "swarm.sqlite".into()),
            http_bind: std::env::var("SWARM_HTTP_BIND")
                .unwrap_or_else(|_| "127.0.0.1:8080".into())
                .parse()
                .expect("SWARM_HTTP_BIND must be host:port"),
            reflector_ports,
            public_url: std::env::var("SWARM_PUBLIC_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
            session_ttl_secs: 30 * 24 * 3600,
            join_code_ttl_secs: 15 * 60,
        }
    }
}
