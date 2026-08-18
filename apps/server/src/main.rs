//! Headless SWARM media server daemon. See `swarm_server::config_from_env`
//! for the environment contract; the Tauri shell (`swarm-server-app`) wraps
//! the same [`swarm_server::ServerCore`].

use std::sync::Arc;

#[tokio::main]
async fn main() {
    // ANSI color codes only help an interactive terminal; a piped/redirected
    // stdout (Docker, systemd, a log file, a test harness parsing this
    // output) should get plain text. tracing-subscriber's own default
    // doesn't reliably auto-detect this in every environment, so check
    // explicitly rather than assume.
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let config = swarm_server::config_from_env().expect("set SWARM_MEDIA_ROOT to your media folder");
    let core = swarm_server::ServerCore::start(config).await.expect("server failed to start");
    tracing::info!(fingerprint = %core.identity.fingerprint, "device identity ready");
    tracing::info!(addr = %core.listen_addr, "peer QUIC listener ready");
    // The initial scan runs in the background (see ServerCore::start's doc
    // comment) so a large library doesn't delay the listener coming up;
    // this just logs once it actually finishes, same info as before, later.
    tokio::spawn({
        let core = Arc::clone(&core);
        async move {
            match core.wait_for_scan().await {
                Ok(report) => tracing::info!(added = report.added, updated = report.updated,
                    removed = report.removed, unchanged = report.unchanged, "library scan complete"),
                Err(err) => tracing::error!(%err, "library scan failed"),
            }
        }
    });

    // One-shot join code, for headless deployments (Docker, a bare VPS) with
    // no GUI to run the onboarding flow. Only fires when nothing is linked
    // yet — a restart with the same env vars after a successful join is a
    // no-op, since `register_with_stun` is never called again.
    if core.stun_link().await.is_none() {
        if let (Ok(base_url), Ok(code)) = (std::env::var("SWARM_STUN_URL"), std::env::var("SWARM_STUN_CODE")) {
            let name = std::env::var("SWARM_DEVICE_NAME").unwrap_or_else(|_| "SWARM Server".into());
            match core.register_with_stun(&base_url, &code, &name).await {
                Ok(swarm) => tracing::info!(swarm = %swarm.name, "joined swarm via SWARM_STUN_CODE"),
                Err(err) => tracing::error!(%err, "SWARM_STUN_CODE join failed; run again with a fresh code"),
            }
        }
    }

    // The accept loop and any roster-sync task run on spawned tasks; park
    // the main task.
    std::future::pending::<()>().await;
}
