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
    /// TV activation codes are intentionally much shorter-lived than device
    /// credentials and are single-use.
    pub activation_ttl_secs: i64,
    /// A managed media server renews this lease whenever it provisions or
    /// authenticates an approval. It is an abuse/garbage-collection bound,
    /// not a client-membership expiry.
    pub managed_swarm_lease_secs: i64,
    pub managed_swarm_max_clients: i64,
    /// `None` when unconfigured — verification/reset links are logged
    /// instead of emailed (the zero-setup dev/test default; see `email.rs`).
    pub smtp: Option<SmtpConfig>,
}

#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// Implicit TLS from connect (port 465-style) vs. STARTTLS (587-style).
    pub implicit_tls: bool,
    pub from_email: String,
    pub from_name: String,
}

impl Config {
    pub fn from_env() -> Self {
        let reflector_ports = std::env::var("SWARM_REFLECTOR_PORTS")
            .unwrap_or_else(|_| "9443,3478".into())
            .split(',')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        Self {
            database_path: std::env::var("SWARM_DATABASE_PATH")
                .unwrap_or_else(|_| "swarm.sqlite".into()),
            http_bind: std::env::var("SWARM_HTTP_BIND")
                .unwrap_or_else(|_| "127.0.0.1:8080".into())
                .parse()
                .expect("SWARM_HTTP_BIND must be host:port"),
            reflector_ports,
            public_url: std::env::var("SWARM_PUBLIC_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
            session_ttl_secs: 30 * 24 * 3600,
            join_code_ttl_secs: 15 * 60,
            activation_ttl_secs: 10 * 60,
            managed_swarm_lease_secs: 30 * 24 * 3600,
            managed_swarm_max_clients: 20,
            smtp: SmtpConfig::from_env(),
        }
    }
}

impl SmtpConfig {
    /// `None` unless `SWARM_SMTP_HOST` is set — that one var gates whether
    /// email is configured at all, matching `reflector_ports` being
    /// empty-to-disable: don't require SMTP just to run tests or `cargo run`.
    fn from_env() -> Option<Self> {
        let host = std::env::var("SWARM_SMTP_HOST").ok()?;
        Some(Self {
            host,
            port: std::env::var("SWARM_SMTP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(465),
            username: std::env::var("SWARM_SMTP_USERNAME").unwrap_or_default(),
            password: std::env::var("SWARM_SMTP_PASSWORD").unwrap_or_default(),
            implicit_tls: std::env::var("SWARM_SMTP_STARTTLS")
                .map(|v| v != "true")
                .unwrap_or(true),
            from_email: std::env::var("SWARM_EMAIL_FROM")
                .expect("SWARM_EMAIL_FROM is required when SWARM_SMTP_HOST is set"),
            from_name: std::env::var("SWARM_EMAIL_FROM_DISPLAY_NAME")
                .unwrap_or_else(|_| "SWARM".into()),
        })
    }
}
