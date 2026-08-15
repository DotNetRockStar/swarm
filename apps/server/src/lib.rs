//! Shared server core: identity → library → scan → pinned QUIC listener,
//! plus STUN swarm membership (register with a join code, keep the QUIC
//! listener's allowed-peer set synced with the swarm roster). Both the
//! headless daemon (`swarm-serverd`) and the Tauri desktop shell
//! (`swarm-server-app`) drive this same surface.

mod stun_link;

use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use swarm_core::rest::{DeviceRegistration, DeviceType, SwarmSummary};
use swarm_media::scan::{scan_root, ScanReport};
use swarm_media::scrape::{run_bulk_scrape, BulkScrapeReport, ScrapeConfig};
use swarm_media::serve::{accept_loop, MediaService};
use swarm_media::store::Library;
use swarm_p2p::identity::DeviceIdentity;
use swarm_p2p::pin::AllowedPeers;
use swarm_stun_client::{StunClient, TokenStore};
use tokio::sync::Mutex;

pub use stun_link::StunLinkRecord;

/// How often a linked server re-fetches its swarms' rosters. Not push-based
/// yet (that lands with WSS presence in Phase 4) — polling is the Phase 2/3
/// stand-in, and it's what keeps a headless daemon's AllowedPeers set fresh
/// even with no GUI open to trigger a manual resync.
const ROSTER_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Where the STUN access token is stored at rest. See
/// `swarm_stun_client::TokenStore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenStoreMode {
    /// Try the OS keychain/credential manager first, falling back to a
    /// permission-restricted file only if no backend is available.
    #[default]
    PreferKeyring,
    /// Skip the keyring entirely — for headless deployments known to have
    /// no Secret Service, and for tests (real keyring behavior varies too
    /// much across environments to assert on reliably).
    FileOnly,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub media_root: PathBuf,
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    /// Fingerprints allowed to connect regardless of STUN membership — for
    /// running without a STUN server at all (local testing, air-gapped use).
    /// A registered STUN link's roster is added on top of this set, never
    /// replacing it.
    pub allowed_fingerprints: Vec<String>,
    pub token_store_mode: TokenStoreMode,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServerStatus {
    pub fingerprint: String,
    pub media_root: String,
    pub listen_addr: String,
    pub entry_count: u64,
    pub thumbprint: String,
}

struct StunContext {
    client: StunClient,
    token_store: TokenStore,
    access_token: String,
    link: StunLinkRecord,
}

pub struct ServerCore {
    pub identity: DeviceIdentity,
    pub library: Arc<Library>,
    pub media_root: PathBuf,
    pub allowed: AllowedPeers,
    pub listen_addr: SocketAddr,
    data_dir: PathBuf,
    /// Fingerprints from `ServerConfig::allowed_fingerprints` — kept
    /// separate so a roster sync can rebuild `allowed` as
    /// `static_fingerprints ∪ swarm_roster` without losing the static set.
    static_fingerprints: Vec<String>,
    token_store_mode: TokenStoreMode,
    stun: Mutex<Option<StunContext>>,
    scraping: AtomicBool,
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
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("STUN server error: {0}")]
    Stun(#[from] swarm_stun_client::StunClientError),
    #[error("token storage error: {0}")]
    TokenStore(#[from] swarm_stun_client::TokenStoreError),
    #[error("a scrape is already running")]
    ScrapeInProgress,
}

impl ServerCore {
    /// Establish identity, open + scan the library, start serving peers, and
    /// restore any previously-established STUN link.
    pub async fn start(config: ServerConfig) -> Result<(Arc<Self>, ScanReport), ServerError> {
        std::fs::create_dir_all(&config.data_dir)?;
        let identity = swarm_p2p::identity::ensure_identity(&config.data_dir)?;
        let library = Arc::new(
            Library::open(config.data_dir.join("library.sqlite").to_str().unwrap_or_default()).await?,
        );
        let report = scan_root(&library, &config.media_root).await?;

        let static_fingerprints: Vec<String> =
            config.allowed_fingerprints.iter().map(|f| f.trim().to_lowercase()).collect();
        let allowed = AllowedPeers::new();
        allowed.replace(static_fingerprints.iter().cloned());
        let endpoint = swarm_p2p::endpoint::listen(config.bind, &identity, allowed.clone())?;
        let listen_addr = endpoint.local_addr()?;
        let service = Arc::new(MediaService::new(Arc::clone(&library), config.media_root.clone()));
        tokio::spawn(accept_loop(endpoint, service));

        let core = Arc::new(Self {
            identity,
            library,
            media_root: config.media_root,
            allowed,
            listen_addr,
            data_dir: config.data_dir,
            static_fingerprints,
            token_store_mode: config.token_store_mode,
            stun: Mutex::new(None),
            scraping: AtomicBool::new(false),
        });
        Arc::clone(&core).restore_stun_link().await;
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

    /// Scrape metadata/artwork for entries that don't have any yet. Rejects
    /// a concurrent call rather than racing two bulk jobs against the same
    /// library (the Drone's module-level-lock discipline).
    pub async fn run_scrape(&self, config: ScrapeConfig) -> Result<BulkScrapeReport, ServerError> {
        if self.scraping.swap(true, Ordering::AcqRel) {
            return Err(ServerError::ScrapeInProgress);
        }
        let cancel = AtomicBool::new(false);
        let result = run_bulk_scrape(&self.library, &self.media_root, &config, &cancel).await;
        self.scraping.store(false, Ordering::Release);
        Ok(result?)
    }

    fn token_store(&self) -> Result<TokenStore, ServerError> {
        let fallback_path = self.data_dir.join("stun-token");
        match self.token_store_mode {
            TokenStoreMode::FileOnly => Ok(TokenStore::file_only(fallback_path)),
            TokenStoreMode::PreferKeyring => {
                Ok(TokenStore::new("swarm-server", &self.identity.fingerprint, fallback_path)?)
            }
        }
    }

    /// Redeem a join code against a STUN server, persist the link + token,
    /// and start keeping `allowed` synced with the swarm roster.
    pub async fn register_with_stun(
        self: &Arc<Self>,
        base_url: &str,
        code: &str,
        device_name: &str,
    ) -> Result<SwarmSummary, ServerError> {
        let machine_id = swarm_stun_client::machine_id::ensure_machine_id(&self.data_dir)?;
        let registration = DeviceRegistration {
            name: device_name.to_string(),
            device_type: DeviceType::Server,
            machine_id,
            cert_fingerprint: self.identity.fingerprint.clone(),
            platform: std::env::consts::OS.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            metadata: BTreeMap::new(),
        };
        let base_url = base_url.trim_end_matches('/').to_string();
        let client = StunClient::new(base_url.clone());
        let response = client.register_device(code, registration).await?;

        let token_store = self.token_store()?;
        token_store.save(&response.access_token)?;
        let link =
            StunLinkRecord { base_url, device_id: response.device_id.clone(), swarms: vec![response.swarm.clone()] };
        stun_link::save(&self.data_dir, &link)?;

        *self.stun.lock().await =
            Some(StunContext { client, token_store, access_token: response.access_token, link });
        Arc::clone(self).spawn_roster_sync_loop();
        self.sync_roster().await?;
        Ok(response.swarm)
    }

    /// Add an already-linked device to another swarm with a fresh code.
    pub async fn join_additional_swarm(&self, code: &str) -> Result<SwarmSummary, ServerError> {
        let swarm = {
            let mut guard = self.stun.lock().await;
            let ctx = guard.as_mut().ok_or(ServerError::Stun(swarm_stun_client::StunClientError::Network(
                "not linked to a STUN server yet".into(),
            )))?;
            let swarm = ctx.client.join_swarm(&ctx.access_token, code).await?;
            ctx.link.swarms.push(swarm.clone());
            stun_link::save(&self.data_dir, &ctx.link)?;
            swarm
        };
        self.sync_roster().await?;
        Ok(swarm)
    }

    /// The currently-linked STUN server and swarms, if any.
    pub async fn stun_link(&self) -> Option<StunLinkRecord> {
        self.stun.lock().await.as_ref().map(|ctx| ctx.link.clone())
    }

    /// Manually trigger a roster re-sync (a GUI "Resync" button, or a test
    /// that doesn't want to wait for `ROSTER_SYNC_INTERVAL`). Also runs on
    /// that fixed schedule automatically while linked. Returns the number of
    /// distinct peer fingerprints now allowed.
    pub async fn resync(&self) -> Result<usize, ServerError> {
        self.sync_roster().await
    }

    async fn restore_stun_link(self: Arc<Self>) {
        let Some(link) = stun_link::load(&self.data_dir) else { return };
        let token_store = match self.token_store() {
            Ok(store) => store,
            Err(err) => {
                tracing::warn!(%err, "could not open token store; STUN link not restored");
                return;
            }
        };
        let access_token = match token_store.load() {
            Ok(Some(token)) => token,
            Ok(None) => {
                tracing::warn!("stun-link.json present but no access token stored; re-registration required");
                return;
            }
            Err(err) => {
                tracing::warn!(%err, "could not read stored access token; re-registration required");
                return;
            }
        };
        let client = StunClient::new(link.base_url.clone());
        *self.stun.lock().await = Some(StunContext { client, token_store, access_token, link });
        tracing::info!("restored STUN link, starting roster sync");
        Arc::clone(&self).spawn_roster_sync_loop();
        if let Err(err) = self.sync_roster().await {
            tracing::debug!(%err, "initial roster sync after restore failed; will retry on schedule");
        }
    }

    fn spawn_roster_sync_loop(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(ROSTER_SYNC_INTERVAL);
            interval.tick().await; // fires immediately; start()/register already did one sync
            loop {
                interval.tick().await;
                if let Err(err) = self.sync_roster().await {
                    tracing::debug!(%err, "swarm roster sync failed; will retry next tick");
                }
            }
        });
    }

    /// Fetch every joined swarm's roster and rebuild `allowed` as
    /// `static_fingerprints ∪ swarm_members` (excluding this device). On any
    /// fetch error the previous `allowed` set is left untouched — a
    /// transient STUN outage must never silently strand connected peers.
    async fn sync_roster(&self) -> Result<usize, ServerError> {
        let guard = self.stun.lock().await;
        let Some(ctx) = guard.as_ref() else { return Ok(0) };
        let mut fingerprints: HashSet<String> = self.static_fingerprints.iter().cloned().collect();
        for swarm in &ctx.link.swarms {
            match ctx.client.swarm_devices(&ctx.access_token, &swarm.id).await {
                Ok(roster) => {
                    for device in roster.devices {
                        if device.cert_fingerprint != self.identity.fingerprint {
                            fingerprints.insert(device.cert_fingerprint);
                        }
                    }
                }
                Err(err) if err.is_unauthorized() => {
                    tracing::warn!(swarm = %swarm.name, "STUN access token was rejected (revoked?); clearing it");
                    let _ = ctx.token_store.delete();
                    return Err(err.into());
                }
                Err(err) => {
                    tracing::debug!(swarm = %swarm.name, %err, "roster fetch failed, keeping previous allowed-peer set");
                    return Err(err.into());
                }
            }
        }
        let count = fingerprints.len();
        self.allowed.replace(fingerprints);
        tracing::debug!(count, "allowed-peer set synced from swarm roster(s)");
        Ok(count)
    }
}

/// Config sourced from env (shared by both binaries):
/// `SWARM_MEDIA_ROOT` (required), `SWARM_DATA_DIR`, `SWARM_PEER_BIND`,
/// `SWARM_ALLOW_FPS` (comma-separated fingerprints, for running without a
/// STUN server at all), `SWARM_TOKEN_STORE_FILE_ONLY` (skip the OS keyring —
/// set this on headless boxes with no Secret Service).
pub fn config_from_env() -> Option<ServerConfig> {
    let media_root = PathBuf::from(std::env::var("SWARM_MEDIA_ROOT").ok()?);
    let file_only = std::env::var("SWARM_TOKEN_STORE_FILE_ONLY")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);
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
        token_store_mode: if file_only { TokenStoreMode::FileOnly } else { TokenStoreMode::PreferKeyring },
    })
}
