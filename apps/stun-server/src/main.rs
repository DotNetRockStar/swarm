//! SWARM STUN server — single tokio process composing:
//! - Axum REST API under `/api/v1` (Swagger UI at `/api/docs`), SQLite (WAL).
//! - WSS signaling hub at `/api/v1/ws` (presence + hole-punch signal routing).
//! - UDP reflectors (`b"bind"` -> `{"ip","port"}`) on the configured ports.
//! - Static web UI (accounts/swarms/codes/devices) served at `/`.
//!
//! Deployed behind Caddy (TLS on 443) via `deploy/docker-compose.yml`.

use std::net::SocketAddr;
use std::sync::Arc;
use stun_server::config::Config;
use stun_server::hub::Hub;
use stun_server::security::BruteForceBlocker;
use stun_server::state::AppState;
use stun_server::{db, reflector, routes};

#[tokio::main]
async fn main() {
    // `--dump-openapi` prints the spec and exits — used by CI to keep
    // openapi/openapi.json (and the generated Kotlin client) in sync.
    if std::env::args().any(|arg| arg == "--dump-openapi") {
        use utoipa::OpenApi;
        println!("{}", routes::ApiDoc::openapi().to_pretty_json().expect("spec serializes"));
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let config = Config::from_env();
    let db = db::connect(&config.database_path).await.expect("failed to open database");
    tracing::info!(path = %config.database_path, "database ready");

    for port in config.reflector_ports.clone() {
        tokio::spawn(async move {
            if let Err(err) = reflector::run(port).await {
                tracing::error!(port, error = %err, "reflector failed");
            }
        });
    }

    let bind = config.http_bind;
    let state = Arc::new(AppState { db, hub: Hub::new(), config, blocker: BruteForceBlocker::new() });
    let static_dir = std::env::var("SWARM_STATIC_DIR").unwrap_or_else(|_| "static".into());
    let static_dir = if std::path::Path::new(&static_dir).join("index.html").exists() {
        Some(static_dir)
    } else {
        tracing::warn!(dir = %static_dir, "static UI directory not found; serving API only");
        None
    };
    let router = routes::build_router(state, static_dir.as_deref());

    let listener = tokio::net::TcpListener::bind(bind).await.expect("failed to bind HTTP listener");
    tracing::info!(%bind, "swarm-stun-server listening (docs at /api/docs)");
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .expect("server error");
}
