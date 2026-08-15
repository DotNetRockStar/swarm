//! Shared server core: identity → library → scan → pinned QUIC listener.
//! Both the headless daemon (`swarm-serverd`) and the Tauri desktop shell
//! (`swarm-server-app`) drive this same surface.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use swarm_media::scan::{scan_root, ScanReport};
use swarm_media::serve::{accept_loop, MediaService};
use swarm_media::store::Library;
use swarm_p2p::identity::DeviceIdentity;
use swarm_p2p::pin::AllowedPeers;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub media_root: PathBuf,
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    /// Fingerprints allowed to connect — the Phase 2 stand-in for the STUN
    /// roster sync that lands with the client work.
    pub allowed_fingerprints: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServerStatus {
    pub fingerprint: String,
    pub media_root: String,
    pub listen_addr: String,
    pub entry_count: u64,
    pub thumbprint: String,
}

pub struct ServerCore {
    pub identity: DeviceIdentity,
    pub library: Arc<Library>,
    pub media_root: PathBuf,
    pub allowed: AllowedPeers,
    pub listen_addr: SocketAddr,
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("identity error: {0}")]
    Identity(#[from] swarm_p2p::identity::IdentityError),
    #[error("library error: {0}")]
    Library(#[from] sqlx::Error),
    #[error("scan error: {0}")]
    Scan(#[from] swarm_media::scan::ScanError),
    #[error("p2p error: {0}")]
    P2p(#[from] swarm_p2p::endpoint::P2pError),
}

impl ServerCore {
    /// Establish identity, open + scan the library, and start serving peers.
    pub async fn start(config: ServerConfig) -> Result<(Self, ScanReport), ServerError> {
        std::fs::create_dir_all(&config.data_dir).map_err(swarm_p2p::identity::IdentityError::Io)?;
        let identity = swarm_p2p::identity::ensure_identity(&config.data_dir)?;
        let library = Arc::new(
            Library::open(config.data_dir.join("library.sqlite").to_str().unwrap_or_default()).await?,
        );
        let report = scan_root(&library, &config.media_root).await?;

        let allowed = AllowedPeers::new();
        allowed.replace(config.allowed_fingerprints.iter().map(|f| f.trim().to_lowercase()));
        let endpoint = swarm_p2p::endpoint::listen(config.bind, &identity, allowed.clone())?;
        let listen_addr = endpoint.local_addr().map_err(swarm_p2p::endpoint::P2pError::Io)?;
        let service = Arc::new(MediaService::new(Arc::clone(&library), config.media_root.clone()));
        tokio::spawn(accept_loop(endpoint, service));

        let core = Self { identity, library, media_root: config.media_root, allowed, listen_addr };
        Ok((core, report))
    }

    pub async fn rescan(&self) -> Result<ScanReport, ServerError> {
        Ok(scan_root(&self.library, &self.media_root).await?)
    }

    pub async fn status(&self) -> Result<ServerStatus, ServerError> {
        Ok(ServerStatus {
            fingerprint: self.identity.fingerprint.clone(),
            media_root: self.media_root.display().to_string(),
            listen_addr: self.listen_addr.to_string(),
            entry_count: self.library.entry_count().await?,
            thumbprint: self.library.thumbprint().await?,
        })
    }
}

/// Config sourced from env (shared by both binaries):
/// `SWARM_MEDIA_ROOT` (required), `SWARM_DATA_DIR`, `SWARM_PEER_BIND`,
/// `SWARM_ALLOW_FPS` (comma-separated fingerprints).
pub fn config_from_env() -> Option<ServerConfig> {
    let media_root = PathBuf::from(std::env::var("SWARM_MEDIA_ROOT").ok()?);
    Some(ServerConfig {
        media_root,
        data_dir: PathBuf::from(std::env::var("SWARM_DATA_DIR").unwrap_or_else(|_| "swarm-server-data".into())),
        bind: std::env::var("SWARM_PEER_BIND")
            .unwrap_or_else(|_| "0.0.0.0:8543".into())
            .parse()
            .expect("SWARM_PEER_BIND must be host:port"),
        allowed_fingerprints: std::env::var("SWARM_ALLOW_FPS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    })
}
