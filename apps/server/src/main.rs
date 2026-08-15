//! Headless SWARM media server daemon. See `swarm_server::config_from_env`
//! for the environment contract; the Tauri shell (`swarm-server-app`) wraps
//! the same [`swarm_server::ServerCore`].

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let config = swarm_server::config_from_env().expect("set SWARM_MEDIA_ROOT to your media folder");
    let (core, report) = swarm_server::ServerCore::start(config).await.expect("server failed to start");
    tracing::info!(fingerprint = %core.identity.fingerprint, "device identity ready");
    tracing::info!(added = report.added, updated = report.updated, removed = report.removed,
        unchanged = report.unchanged, "library scan complete");
    tracing::info!(addr = %core.listen_addr, "peer QUIC listener ready");
    // The accept loop runs on spawned tasks; park the main task.
    std::future::pending::<()>().await;
}
