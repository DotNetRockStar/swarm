//! Headless SWARM media server daemon (Phase 2 core; the Tauri shell wraps
//! this same library surface later).
//!
//! Env:
//! - `SWARM_MEDIA_ROOT`   library root to scan (required)
//! - `SWARM_DATA_DIR`     identity + library DB location (default ./swarm-server-data)
//! - `SWARM_PEER_BIND`    QUIC listen address (default 0.0.0.0:8543)
//! - `SWARM_ALLOW_FPS`    comma-separated client cert fingerprints allowed to
//!   connect (Phase 2 stand-in for the STUN roster sync landing in Phase 3)

use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let media_root = std::env::var("SWARM_MEDIA_ROOT").expect("set SWARM_MEDIA_ROOT to your media folder");
    let data_dir = std::env::var("SWARM_DATA_DIR").unwrap_or_else(|_| "swarm-server-data".into());
    let bind: std::net::SocketAddr = std::env::var("SWARM_PEER_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8543".into())
        .parse()
        .expect("SWARM_PEER_BIND must be host:port");

    let identity = swarm_p2p::identity::ensure_identity(std::path::Path::new(&data_dir))
        .expect("failed to establish device identity");
    tracing::info!(fingerprint = %identity.fingerprint, "device identity ready");

    let library = swarm_media::store::Library::open(&format!("{data_dir}/library.sqlite"))
        .await
        .expect("failed to open library database");
    let report = swarm_media::scan::scan_root(&library, std::path::Path::new(&media_root))
        .await
        .expect("library scan failed");
    tracing::info!(added = report.added, updated = report.updated, removed = report.removed,
        unchanged = report.unchanged, "library scan complete");

    let allowed = swarm_p2p::pin::AllowedPeers::new();
    let fps = std::env::var("SWARM_ALLOW_FPS").unwrap_or_default();
    allowed.replace(fps.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()));

    let endpoint = swarm_p2p::endpoint::listen(bind, &identity, allowed).expect("failed to bind QUIC listener");
    tracing::info!(%bind, "peer QUIC listener ready");
    let service = Arc::new(swarm_media::serve::MediaService::new(
        library,
        std::path::PathBuf::from(&media_root),
    ));
    swarm_media::serve::accept_loop(endpoint, service).await;
}
