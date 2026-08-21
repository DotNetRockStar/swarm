//! Shared server core: identity → library → scan → pinned QUIC listener,
//! plus SWARM membership (register with a join code, keep the QUIC listener's
//! allowed-peer set synced with the swarm roster). The Tauri desktop app owns
//! this core for its entire process lifetime, including while hidden to tray.

mod bandwidth;
pub mod lan;
pub mod punch_connect;
mod state_db;
pub mod transcription;

use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use swarm_core::peer::MediaKind;
use swarm_core::rest::{
    ActivationPreview, ActivationStatusResponse, DeviceRegistration, DeviceType,
    ProvisionManagedSwarmRequest, SwarmDevicesResponse, SwarmSummary,
};
use swarm_core::signal::{SignalMessage, SignalPayload};
use swarm_media::roots::{MediaRoot, RootResolver, SharedRootResolver};
use swarm_media::scan::{scan_roots, ScanProgressEvent, ScanReport};
use swarm_media::scrape::{
    run_bulk_scrape, scrape_one_track, scrape_one_video, BulkScrapeReport, ScrapeConfig,
    ScrapeOneError, ScrapeProgressEvent, TmdbOverride,
};
use swarm_media::serve::{accept_loop, serve_connection, MediaService};
use swarm_media::store::Library;
use swarm_media::transcode::TranscodeConfig;
use swarm_p2p::identity::DeviceIdentity;
use swarm_p2p::pin::AllowedPeers;
use swarm_stun_client::{SignalingClient, StunClient, TokenStore};
use tokio::sync::mpsc;
use tokio::sync::Mutex;

use crate::punch_connect::{respond_to_punch_offer, ReceivedOffer};
use crate::transcription::{TranscriptionManager, TranscriptionStatus};

pub use state_db::{LocalPeerRecord, ManagedSwarmIdentity, StunLinkRecord};

/// How often a linked server re-fetches its swarms' rosters. Not push-based
/// yet (that lands with WSS presence in Phase 4) — polling is the Phase 2/3
/// stand-in, and it keeps AllowedPeers fresh even while the desktop window is
/// hidden and no GUI action can trigger a manual resync.
const ROSTER_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Where the STUN access token is stored at rest. See
/// `swarm_stun_client::TokenStore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenStoreMode {
    /// Try the OS keychain/credential manager first, falling back to a
    /// permission-restricted file only if no backend is available.
    #[default]
    PreferKeyring,
    /// Skip the keyring entirely — used by the desktop app so unsigned local
    /// rebuilds retain access, and by tests because keyring behavior varies
    /// too much across environments to assert on reliably.
    FileOnly,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// One or more library roots. A single root behaves exactly as before
    /// (its `label` is never written onto `relative_path`); 2+ roots are
    /// distinguished on-disk by a `{label}/` prefix — see
    /// `swarm_media::roots`.
    pub media_roots: Vec<MediaRoot>,
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    /// Fingerprints allowed to connect regardless of STUN membership — for
    /// running without a STUN server at all (local testing, air-gapped use).
    /// A registered STUN link's roster is added on top of this set, never
    /// replacing it.
    pub allowed_fingerprints: Vec<String>,
    pub token_store_mode: TokenStoreMode,
    /// Public SWARM service used to create or renew the server-owned swarm.
    /// `None` preserves a legacy/manual link unless a managed identity was
    /// already created locally, in which case that identity is still renewed.
    pub managed_rendezvous_url: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServerStatus {
    pub fingerprint: String,
    /// `"label: /absolute/path"` per configured root (single-root installs
    /// still get one entry here, just with no prefix applied on-disk).
    pub media_roots: Vec<String>,
    pub listen_addr: String,
    pub entry_count: u64,
    pub thumbprint: String,
    pub streaming_upload_budget_bps: u64,
    pub streaming_upload_budget_enabled: bool,
    pub active_playback_sessions: usize,
    /// True while a scan (initial, rescan, or a root change) is in
    /// progress — the library reflects whatever's been found so far either
    /// way, this is purely informational for a "still scanning…" indicator.
    pub scanning: bool,
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
    pub media_roots: SharedRootResolver,
    pub allowed: AllowedPeers,
    pub listen_addr: SocketAddr,
    service: Arc<MediaService>,
    transcription: Arc<TranscriptionManager>,
    data_dir: PathBuf,
    state_db: Arc<state_db::StateDb>,
    lan_service: lan::LanService,
    /// Fingerprints from `ServerConfig::allowed_fingerprints` — kept
    /// separate so a roster sync can rebuild `allowed` as
    /// `static_fingerprints ∪ swarm_roster` without losing the static set.
    static_fingerprints: Vec<String>,
    token_store_mode: TokenStoreMode,
    stun: Mutex<Option<StunContext>>,
    scraping: AtomicBool,
    /// Serializes every full scan (the initial background one, `rescan`, and
    /// `update_media_roots`) — `scan_roots` snapshots known entries then
    /// walks and reconciles based on that snapshot, so two overlapping scans
    /// of the same root set could race each other's reconciliation and
    /// resurrect/delete entries incorrectly. A later caller simply waits its
    /// turn rather than being rejected.
    scan_lock: tokio::sync::Mutex<()>,
    scan_status: tokio::sync::watch::Sender<ScanState>,
}

/// The current/last outcome of `ServerCore`'s background or on-demand
/// scanning — see [`ServerCore::start`]'s doc comment for why the initial
/// scan doesn't block startup, and [`ServerCore::wait_for_scan`] for how a
/// caller that specifically needs completion (tests, mainly) can get it.
#[derive(Debug, Clone, Default)]
pub enum ScanState {
    #[default]
    NotStarted,
    Scanning,
    Done(ScanReport),
    Failed(String),
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
    #[error("SWARM server error: {0}")]
    Stun(#[from] swarm_stun_client::StunClientError),
    #[error("token storage error: {0}")]
    TokenStore(#[from] swarm_stun_client::TokenStoreError),
    #[error("a scrape is already running")]
    ScrapeInProgress,
    #[error("scrape error: {0}")]
    Scrape(#[from] ScrapeOneError),
    #[error("no library entry with that key")]
    EntryNotFound,
    #[error("at least one media root is required")]
    NoMediaRoots,
}

impl ServerCore {
    /// Establish identity, open the library, start serving peers, and
    /// restore any previously-established STUN link — all synchronously.
    /// The initial library scan is spawned in the background rather than
    /// awaited here: a real user's library can be tens of thousands of
    /// files on a network share, taking many minutes to walk, and every
    /// Tauri command touches this same `Arc<ServerCore>` — awaiting the scan
    /// inline meant the very first command after launch (and, since the GUI
    /// builds this core lazily behind a `OnceCell`, therefore *every*
    /// command from *every* tab) blocked on the entire scan before the app
    /// could respond to anything at all. Callers that specifically need the
    /// initial scan's result (mainly tests) can await [`Self::wait_for_scan`].
    pub async fn start(config: ServerConfig) -> Result<Arc<Self>, ServerError> {
        let configured_managed_url = config
            .managed_rendezvous_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches('/').to_string());
        std::fs::create_dir_all(&config.data_dir)?;
        let identity = swarm_p2p::identity::ensure_identity(&config.data_dir)?;
        let library = Arc::new(
            Library::open(
                config
                    .data_dir
                    .join("library.sqlite")
                    .to_str()
                    .unwrap_or_default(),
            )
            .await?,
        );
        let state_db = Arc::new(state_db::StateDb::open(&config.data_dir).await?);
        let media_roots = SharedRootResolver::new(RootResolver::new(config.media_roots));

        let static_fingerprints: Vec<String> = config
            .allowed_fingerprints
            .iter()
            .map(|f| f.trim().to_lowercase())
            .collect();
        let allowed = AllowedPeers::new();
        let local_fingerprints = state_db
            .local_peers()
            .await?
            .into_iter()
            .map(|peer| peer.fingerprint);
        allowed.replace(
            static_fingerprints
                .iter()
                .cloned()
                .chain(local_fingerprints),
        );
        let endpoint = swarm_p2p::endpoint::listen(config.bind, &identity, allowed.clone())?;
        let listen_addr = endpoint.local_addr()?;
        let lan_service = lan::LanService::start(
            identity.fingerprint.clone(),
            listen_addr,
            allowed.clone(),
            Arc::clone(&state_db),
        )
        .await?;

        // Seed the streaming budget from the last real measurement (if
        // any) right at construction, so a restart doesn't fall back to
        // the static default for a full probe interval before its first
        // tick completes — see bandwidth.rs.
        let mut transcode_config = transcode_config_from_env(&config.data_dir);
        if let Some(measured_bps) = state_db.latest_bandwidth_measurement().await? {
            transcode_config.max_upload_bps = measured_bps;
        }
        let ffmpeg_path = transcode_config.ffmpeg_path.clone();
        let service = Arc::new(MediaService::with_roots(
            Arc::clone(&library),
            media_roots.clone(),
            transcode_config,
        ));
        tokio::spawn(accept_loop(endpoint, Arc::clone(&service)));
        tokio::spawn(bandwidth::run_periodic_probe(
            Arc::clone(&state_db),
            Arc::clone(service.transcode_manager()),
            bandwidth::interval_from_env(),
        ));
        let transcription = TranscriptionManager::start(
            Arc::clone(&library),
            media_roots.clone(),
            Arc::clone(service.transcode_manager()),
            &config.data_dir,
            ffmpeg_path,
        )
        .await?;

        let core = Arc::new(Self {
            identity,
            library,
            media_roots,
            allowed,
            listen_addr,
            service,
            transcription,
            data_dir: config.data_dir,
            state_db,
            lan_service,
            static_fingerprints,
            token_store_mode: config.token_store_mode,
            stun: Mutex::new(None),
            scraping: AtomicBool::new(false),
            scan_lock: tokio::sync::Mutex::new(()),
            scan_status: tokio::sync::watch::Sender::new(ScanState::NotStarted),
        });
        // A configured or previously-created managed swarm takes precedence
        // over an old manual link. Previously this restored the old link first
        // and skipped provisioning whenever *any* link existed. The resulting
        // token could browse a normal swarm but did not own a managed one, so
        // TV activation lookup/approval failed with 403.
        let stored_managed_url = core
            .state_db
            .load_managed_swarm_identity()
            .await?
            .map(|identity| identity.base_url);
        let managed_url = configured_managed_url.or(stored_managed_url);
        let mut managed_ready = false;
        if let Some(base_url) = managed_url {
            let name =
                std::env::var("SWARM_DEVICE_NAME").unwrap_or_else(|_| "SWARM Media Server".into());
            match Arc::clone(&core)
                .provision_managed_swarm(&base_url, &name)
                .await
            {
                Ok(_) => managed_ready = true,
                Err(err) => {
                    tracing::warn!(%err, "automatic SWARM provisioning failed; trying the saved link");
                }
            }
        }
        if !managed_ready {
            Arc::clone(&core).restore_stun_link().await;
        }

        // Mark Scanning synchronously, before returning, so a caller that
        // calls wait_for_scan() immediately after start() can never observe
        // the pre-scan NotStarted default and return early.
        core.scan_status.send_modify(|s| *s = ScanState::Scanning);
        let scan_core = Arc::clone(&core);
        tokio::spawn(async move {
            let roots = scan_core.media_roots.roots();
            match scan_core.run_scan(&roots, None).await {
                Ok(report) => tracing::info!(
                    added = report.added,
                    updated = report.updated,
                    removed = report.removed,
                    unchanged = report.unchanged,
                    "initial library scan complete"
                ),
                Err(err) => tracing::error!(%err, "initial library scan failed"),
            }
        });

        Ok(core)
    }

    /// Runs one full scan, serialized against every other scan on this core
    /// (initial, rescan, or a root change) via `scan_lock` — see the lock's
    /// doc comment on `Self` for why concurrent scans of the same root set
    /// would be unsafe, not just wasteful. Updates `scan_status` throughout.
    async fn run_scan(
        &self,
        roots: &[MediaRoot],
        progress_tx: Option<mpsc::UnboundedSender<ScanProgressEvent>>,
    ) -> Result<ScanReport, ServerError> {
        let _guard = self.scan_lock.lock().await;
        self.scan_status.send_modify(|s| *s = ScanState::Scanning);
        match scan_roots(&self.library, roots, progress_tx).await {
            Ok(report) => {
                self.scan_status
                    .send_modify(|s| *s = ScanState::Done(report.clone()));
                Ok(report)
            }
            Err(err) => {
                self.scan_status
                    .send_modify(|s| *s = ScanState::Failed(err.to_string()));
                Err(err.into())
            }
        }
    }

    /// The current/last scan outcome — see [`ScanState`].
    pub fn scan_status(&self) -> ScanState {
        self.scan_status.borrow().clone()
    }

    /// Blocks until the current or next scan to complete (or fail) finishes,
    /// returning its report. Mainly for tests that need deterministic
    /// post-scan assertions — regular callers should just read whatever the
    /// library currently has via `scan_status()`/`library.list()` and let it
    /// catch up live, the same way the scrape-progress UI already does.
    pub async fn wait_for_scan(&self) -> Result<ScanReport, String> {
        let mut rx = self.scan_status.subscribe();
        loop {
            match &*rx.borrow() {
                ScanState::Done(report) => return Ok(report.clone()),
                ScanState::Failed(err) => return Err(err.clone()),
                ScanState::NotStarted | ScanState::Scanning => {}
            }
            rx.changed()
                .await
                .expect("ServerCore dropped its own scan_status sender");
        }
    }

    pub async fn rescan(
        &self,
        progress_tx: Option<mpsc::UnboundedSender<ScanProgressEvent>>,
    ) -> Result<ScanReport, ServerError> {
        let roots = self.media_roots.roots();
        self.run_scan(&roots, progress_tx).await
    }

    /// Live-swap the configured media roots and immediately reconcile the
    /// library against them — no restart required. Shared by every caller
    /// (`ServerCore`'s scan/scrape paths and `MediaService`'s P2P
    /// serving/artwork paths) all observe the new roots on their very next
    /// call, since they hold clones of the same [`SharedRootResolver`]
    /// handle rather than independent copies.
    ///
    /// Reconciliation reuses [`scan_roots`] unchanged: it snapshots every
    /// currently-known entry, walks only the roots passed in, and removes
    /// anything not seen during that walk. Pointing it at a different root
    /// set than the one that produced the current library state therefore
    /// already does the right thing — entries from a removed root are found
    /// nowhere during the walk and get removed exactly like a deleted file
    /// would, with no special-cased "root disappeared" handling needed.
    pub async fn update_media_roots(
        &self,
        roots: Vec<MediaRoot>,
    ) -> Result<ScanReport, ServerError> {
        if roots.is_empty() {
            return Err(ServerError::NoMediaRoots);
        }
        self.media_roots.replace(roots.clone());
        self.run_scan(&roots, None).await
    }

    pub async fn status(&self) -> Result<ServerStatus, ServerError> {
        Ok(ServerStatus {
            fingerprint: self.identity.fingerprint.clone(),
            media_roots: self
                .media_roots
                .roots()
                .iter()
                .map(|root| format!("{}: {}", root.label, root.path.display()))
                .collect(),
            listen_addr: self.listen_addr.to_string(),
            entry_count: self.library.entry_count().await?,
            thumbprint: self.library.thumbprint().await?,
            streaming_upload_budget_bps: self.service.transcode_manager().usable_upload_bps(),
            streaming_upload_budget_enabled: self
                .service
                .transcode_manager()
                .upload_budget_enabled(),
            active_playback_sessions: self.service.transcode_manager().active_sessions(),
            scanning: matches!(&*self.scan_status.borrow(), ScanState::Scanning),
        })
    }

    /// Enables or pauses the durable local subtitle worker. Pausing is
    /// cooperative and preserves every completed ten-minute segment.
    pub fn set_local_transcription_enabled(&self, enabled: bool) {
        self.transcription.set_enabled(enabled);
    }

    pub async fn transcription_status(&self) -> Result<TranscriptionStatus, ServerError> {
        Ok(self.transcription.status().await?)
    }

    /// Live preference used by the desktop app. LAN connections always
    /// bypass the budget in `swarm_media::serve`, even when this is true.
    pub fn set_streaming_upload_budget_enabled(&self, enabled: bool) {
        self.service
            .transcode_manager()
            .set_upload_budget_enabled(enabled);
    }

    /// Scrape metadata/artwork for entries that don't have any yet. Rejects
    /// a concurrent call rather than racing two bulk jobs against the same
    /// library (the Drone's module-level-lock discipline). `progress_tx`,
    /// when given, receives one [`ScrapeProgressEvent`] per entry as it
    /// completes — entirely optional so this method still works exactly as
    /// before for any caller that doesn't need live updates.
    /// `force`: re-scrape and overwrite every entry, not just ones missing a
    /// scrape result — the UI's "redownload / override existing" checkbox.
    pub async fn run_scrape(
        &self,
        config: ScrapeConfig,
        progress_tx: Option<mpsc::UnboundedSender<ScrapeProgressEvent>>,
        force: bool,
    ) -> Result<BulkScrapeReport, ServerError> {
        if self.scraping.swap(true, Ordering::AcqRel) {
            return Err(ServerError::ScrapeInProgress);
        }
        let cancel = AtomicBool::new(false);
        let result = run_bulk_scrape(
            &self.library,
            &self.media_roots,
            &config,
            &cancel,
            progress_tx,
            force,
        )
        .await;
        self.scraping.store(false, Ordering::Release);
        Ok(result?)
    }

    /// Pinpoint rescrape of one entry — unlike [`Self::run_scrape`], this
    /// succeeds even on an already-scraped entry (correcting a wrong match
    /// is the whole point) and is not gated by the bulk-scrape-in-progress
    /// guard, since it's a single targeted lookup rather than a library-wide
    /// job. `tmdb_override` is ignored for music entries (no TMDb concept
    /// there); a track rescrape always re-syncs its whole (artist, album)
    /// group, matching bulk behavior.
    pub async fn rescrape_entry(
        &self,
        entry_key: &str,
        config: ScrapeConfig,
        tmdb_override: Option<TmdbOverride>,
    ) -> Result<(), ServerError> {
        let entry = self
            .library
            .get(entry_key)
            .await?
            .ok_or(ServerError::EntryNotFound)?;
        match entry.kind {
            MediaKind::Track => {
                scrape_one_track(&self.library, &self.media_roots, &config, &entry).await?;
            }
            MediaKind::Movie | MediaKind::Episode => {
                scrape_one_video(
                    &self.library,
                    &self.media_roots,
                    &config,
                    &entry,
                    tmdb_override,
                )
                .await?;
            }
        }
        Ok(())
    }

    fn token_store(&self) -> Result<TokenStore, ServerError> {
        let fallback_path = self.data_dir.join("stun-token");
        match self.token_store_mode {
            TokenStoreMode::FileOnly => Ok(TokenStore::file_only(fallback_path)),
            TokenStoreMode::PreferKeyring => Ok(TokenStore::new(
                "swarm-server",
                &self.identity.fingerprint,
                fallback_path,
            )?),
        }
    }

    fn managed_claim_store(&self) -> Result<TokenStore, ServerError> {
        let fallback_path = self.data_dir.join("managed-swarm-claim");
        match self.token_store_mode {
            TokenStoreMode::FileOnly => Ok(TokenStore::file_only(fallback_path)),
            TokenStoreMode::PreferKeyring => Ok(TokenStore::new(
                "swarm-server-managed-owner",
                &self.identity.fingerprint,
                fallback_path,
            )?),
        }
    }

    fn server_registration(&self, device_name: &str, machine_id: String) -> DeviceRegistration {
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "peer_addr".to_string(),
            swarm_p2p::local_addr::detect_local_addr(self.listen_addr.port()).to_string(),
        );
        DeviceRegistration {
            name: device_name.to_string(),
            device_type: DeviceType::Server,
            machine_id,
            cert_fingerprint: self.identity.fingerprint.clone(),
            platform: std::env::consts::OS.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            metadata,
        }
    }

    /// Idempotently creates or renews the private swarm this media server
    /// owns. The claim secret lives in a separate OS credential entry (or a
    /// 0600 fallback file), never in SQLite.
    pub async fn provision_managed_swarm(
        self: Arc<Self>,
        base_url: &str,
        device_name: &str,
    ) -> Result<SwarmSummary, ServerError> {
        let base_url = base_url.trim_end_matches('/').to_string();
        let claim_store = self.managed_claim_store()?;
        let existing = self.state_db.load_managed_swarm_identity().await?;
        let (identity, claim_token) = match existing {
            Some(identity) => {
                let claim = claim_store.load()?.ok_or_else(|| {
                    ServerError::Stun(swarm_stun_client::StunClientError::Decode(
                        "managed swarm identity exists but its owner credential is missing".into(),
                    ))
                })?;
                (identity, claim)
            }
            None => {
                let identity = ManagedSwarmIdentity {
                    base_url: base_url.clone(),
                    swarm_id: swarm_stun_client::random_token(),
                };
                let claim = swarm_stun_client::random_token();
                claim_store.save(&claim)?;
                self.state_db.save_managed_swarm_identity(&identity).await?;
                (identity, claim)
            }
        };
        if identity.base_url.trim_end_matches('/') != base_url {
            return Err(ServerError::Stun(
                swarm_stun_client::StunClientError::Decode(
                    "this server's managed swarm belongs to a different SWARM service".into(),
                ),
            ));
        }
        let machine_id = swarm_stun_client::machine_id::ensure_machine_id(&self.data_dir)?;
        let registration = self.server_registration(device_name, machine_id);
        let client = StunClient::new(base_url.clone());
        let response = client
            .provision_managed_swarm(ProvisionManagedSwarmRequest {
                swarm_id: identity.swarm_id,
                claim_token,
                swarm_name: format!("{}'s SWARM", device_name.trim()),
                device: registration,
            })
            .await?;
        let token_store = self.token_store()?;
        token_store.save(&response.access_token)?;
        let link = StunLinkRecord {
            base_url,
            device_id: response.device_id.clone(),
            swarms: vec![response.swarm.clone()],
        };
        self.state_db.save_stun_link(&link).await?;
        self.establish_signaling(&link.base_url, &response.access_token, &link.device_id)
            .await;
        *self.stun.lock().await = Some(StunContext {
            client,
            token_store,
            access_token: response.access_token,
            link,
        });
        Arc::clone(&self).spawn_roster_sync_loop();
        self.sync_roster().await?;
        Ok(response.swarm)
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
        // Submitted immediately so a client checking the roster right after
        // this server joins doesn't have to wait for the first periodic
        // sync tick to learn where to dial it — see sync_roster for the
        // ongoing refresh.
        let registration = self.server_registration(device_name, machine_id);
        let base_url = base_url.trim_end_matches('/').to_string();
        let client = StunClient::new(base_url.clone());
        let response = client.register_device(code, registration).await?;

        let token_store = self.token_store()?;
        token_store.save(&response.access_token)?;
        let link = StunLinkRecord {
            base_url,
            device_id: response.device_id.clone(),
            swarms: vec![response.swarm.clone()],
        };
        self.state_db.save_stun_link(&link).await?;

        self.establish_signaling(&link.base_url, &response.access_token, &link.device_id)
            .await;
        *self.stun.lock().await = Some(StunContext {
            client,
            token_store,
            access_token: response.access_token,
            link,
        });
        Arc::clone(self).spawn_roster_sync_loop();
        self.sync_roster().await?;
        Ok(response.swarm)
    }

    /// Add an already-linked device to another swarm with a fresh code.
    pub async fn join_additional_swarm(&self, code: &str) -> Result<SwarmSummary, ServerError> {
        let swarm = {
            let mut guard = self.stun.lock().await;
            let ctx = guard.as_mut().ok_or(ServerError::Stun(
                swarm_stun_client::StunClientError::Network(
                    "not linked to a SWARM server yet".into(),
                ),
            ))?;
            let swarm = ctx.client.join_swarm(&ctx.access_token, code).await?;
            ctx.link.swarms.push(swarm.clone());
            self.state_db.save_stun_link(&ctx.link).await?;
            swarm
        };
        self.sync_roster().await?;
        Ok(swarm)
    }

    /// Leave one swarm this server belongs to, keeping the STUN link (and
    /// its other swarm memberships) intact — symmetric with
    /// `join_additional_swarm`. Shrinks `allowed` via the roster resync
    /// that follows.
    pub async fn leave_swarm(&self, swarm_id: &str) -> Result<(), ServerError> {
        {
            let mut guard = self.stun.lock().await;
            let ctx = guard.as_mut().ok_or(ServerError::Stun(
                swarm_stun_client::StunClientError::Network(
                    "not linked to a SWARM server yet".into(),
                ),
            ))?;
            ctx.client
                .leave_swarm(&ctx.access_token, swarm_id, &ctx.link.device_id)
                .await?;
            ctx.link.swarms.retain(|s| s.id != swarm_id);
            self.state_db.save_stun_link(&ctx.link).await?;
        }
        self.sync_roster().await?;
        Ok(())
    }

    /// The currently-linked STUN server and swarms, if any.
    pub async fn stun_link(&self) -> Option<StunLinkRecord> {
        self.stun.lock().await.as_ref().map(|ctx| ctx.link.clone())
    }

    /// One joined swarm's device roster — a straight passthrough to the STUN
    /// server's own view, for display in a GUI. Unlike `sync_roster`, this
    /// never touches `allowed`; it's read-only from this device's
    /// perspective.
    pub async fn swarm_devices(&self, swarm_id: &str) -> Result<SwarmDevicesResponse, ServerError> {
        let guard = self.stun.lock().await;
        let ctx = guard.as_ref().ok_or(ServerError::Stun(
            swarm_stun_client::StunClientError::Network("not linked to a SWARM server yet".into()),
        ))?;
        Ok(ctx
            .client
            .swarm_devices(&ctx.access_token, swarm_id)
            .await?)
    }

    pub async fn lookup_activation(&self, code: &str) -> Result<ActivationPreview, ServerError> {
        let guard = self.stun.lock().await;
        let ctx = guard.as_ref().ok_or(ServerError::Stun(
            swarm_stun_client::StunClientError::Network("not linked to a SWARM service yet".into()),
        ))?;
        Ok(ctx
            .client
            .lookup_activation(&ctx.access_token, code)
            .await?)
    }

    pub async fn approve_activation(
        &self,
        activation_id: &str,
    ) -> Result<ActivationStatusResponse, ServerError> {
        let guard = self.stun.lock().await;
        let ctx = guard.as_ref().ok_or(ServerError::Stun(
            swarm_stun_client::StunClientError::Network("not linked to a SWARM service yet".into()),
        ))?;
        let result = ctx
            .client
            .approve_activation(&ctx.access_token, activation_id)
            .await?;
        drop(guard);
        self.sync_roster().await?;
        Ok(result)
    }

    /// Manually trigger a roster re-sync (a GUI "Resync" button, or a test
    /// that doesn't want to wait for `ROSTER_SYNC_INTERVAL`). Also runs on
    /// that fixed schedule automatically while linked. Returns the number of
    /// distinct peer fingerprints now allowed.
    pub async fn resync(&self) -> Result<usize, ServerError> {
        self.sync_roster().await
    }

    /// Opens the time-limited, single-use LAN pairing window advertised over
    /// mDNS. Discovery remains available all the time; only authorization is
    /// gated by this explicit user action.
    pub async fn open_lan_pairing(&self) -> lan::PairingStatus {
        self.lan_service.open_pairing_window().await
    }

    pub async fn local_peers(&self) -> Result<Vec<LocalPeerRecord>, ServerError> {
        Ok(self.state_db.local_peers().await?)
    }

    pub async fn revoke_local_peer(&self, fingerprint: &str) -> Result<(), ServerError> {
        self.state_db.remove_local_peer(fingerprint).await?;
        self.sync_roster().await?;
        Ok(())
    }

    async fn restore_stun_link(self: Arc<Self>) {
        let Some(link) = self.state_db.load_stun_link().await.unwrap_or_else(|err| {
            tracing::warn!(%err, "could not read saved STUN link; starting unlinked");
            None
        }) else {
            return;
        };
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
                tracing::warn!(
                    "stun-link.json present but no access token stored; re-registration required"
                );
                return;
            }
            Err(err) => {
                tracing::warn!(%err, "could not read stored access token; re-registration required");
                return;
            }
        };
        let client = StunClient::new(link.base_url.clone());
        self.establish_signaling(&link.base_url, &access_token, &link.device_id)
            .await;
        *self.stun.lock().await = Some(StunContext {
            client,
            token_store,
            access_token,
            link,
        });
        tracing::info!("restored STUN link, starting roster sync");
        Arc::clone(&self).spawn_roster_sync_loop();
        if let Err(err) = self.sync_roster().await {
            tracing::debug!(%err, "initial roster sync after restore failed; will retry on schedule");
        }
    }

    /// Opens a signaling session and, if that succeeds, resolves the
    /// reflector's address and starts the punch-dispatch loop. Best-effort
    /// and never fatal to the caller: a server with no working signaling
    /// session still serves LAN direct-play peers via `peer_addr` just
    /// fine, it just can't accept a connection from anyone off-LAN —
    /// logged, not propagated as an error.
    async fn establish_signaling(
        self: &Arc<Self>,
        base_url: &str,
        access_token: &str,
        device_id: &str,
    ) {
        let (signaling, signal_rx) = match SignalingClient::connect(
            base_url,
            access_token,
            device_id,
            None,
        )
        .await
        {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(%err, "could not open a signaling session; hole-punch connections unavailable on this link");
                return;
            }
        };
        let Some(reflector_addr) =
            resolve_reflector_addr(base_url, &signaling.reflector_ports).await
        else {
            tracing::warn!("could not resolve the reflector's address; hole-punch connections unavailable on this link");
            return;
        };
        Arc::clone(self).spawn_punch_dispatch_loop(signaling, signal_rx, reflector_addr);
    }

    /// Owns the signaling receiver for as long as this link lives: reacts to
    /// an incoming `Offer` from a swarm-mate by answering it, punching, and
    /// — once mutually confirmed — serving the resulting QUIC connection
    /// exactly like one accepted on the main listener. Everything else
    /// (presence, stray signals) is ignored; nothing else on this server
    /// reads from this receiver, so there's no contention to design around.
    ///
    /// Known limitation, not solved here: only one punch negotiation runs
    /// at a time, since answering one offer borrows this receiver until
    /// that attempt finishes or times out (see `punch_connect`'s module
    /// doc). A second peer's offer arriving mid-negotiation sits in the
    /// channel until the first attempt is done, rather than being handled
    /// concurrently.
    fn spawn_punch_dispatch_loop(
        self: Arc<Self>,
        signaling: SignalingClient,
        mut signal_rx: mpsc::UnboundedReceiver<SignalMessage>,
        reflector_addr: SocketAddr,
    ) {
        tokio::spawn(async move {
            loop {
                let message = match signal_rx.recv().await {
                    Some(message) => message,
                    None => {
                        tracing::debug!("signaling session closed; no longer accepting hole-punched connections");
                        return;
                    }
                };
                let SignalMessage::Signal {
                    from: Some(from),
                    payload:
                        SignalPayload::Offer {
                            punch_id,
                            candidates,
                            cert_fingerprint,
                        },
                    ..
                } = message
                else {
                    continue;
                };
                let offer = ReceivedOffer {
                    from: from.clone(),
                    punch_id,
                    candidates,
                    cert_fingerprint,
                };
                match respond_to_punch_offer(
                    &signaling,
                    &mut signal_rx,
                    reflector_addr,
                    offer,
                    &self.identity,
                    self.allowed.clone(),
                )
                .await
                {
                    Ok(connection) => {
                        tracing::info!(peer = %from, "hole-punched connection established");
                        tokio::spawn(serve_connection(connection, Arc::clone(&self.service)));
                    }
                    Err(err) => {
                        tracing::debug!(peer = %from, %err, "hole-punch negotiation failed")
                    }
                }
            }
        });
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
        let mut fingerprints: HashSet<String> = self.static_fingerprints.iter().cloned().collect();
        fingerprints.extend(
            self.state_db
                .local_peers()
                .await?
                .into_iter()
                .map(|peer| peer.fingerprint),
        );
        let Some(ctx) = guard.as_ref() else {
            let count = fingerprints.len();
            self.allowed.replace(fingerprints);
            return Ok(count);
        };

        // Best-effort: keep the connectable address peers see on the STUN
        // roster fresh (DHCP renewal, wifi reconnect, ...). Never blocks or
        // fails the allowed-peer sync below — a stale address just means a
        // client's next connect attempt uses last-known-good info, same
        // spirit as the peer route memory the Kotlin/Rust P2P clients keep.
        let mut self_metadata = BTreeMap::new();
        self_metadata.insert(
            "peer_addr".to_string(),
            swarm_p2p::local_addr::detect_local_addr(self.listen_addr.port()).to_string(),
        );
        if let Err(err) = ctx
            .client
            .patch_metadata(&ctx.access_token, &ctx.link.device_id, self_metadata)
            .await
        {
            tracing::debug!(%err, "failed to self-report peer address this cycle");
        }

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

/// The reflector runs inside the STUN server process (`docs/PROTOCOL.md`),
/// so its address is the STUN base URL's host plus whichever port
/// `hello_ack` advertised as live — resolved via DNS since the host in a
/// base URL is as likely to be a domain name as a literal IP.
async fn resolve_reflector_addr(base_url: &str, reflector_ports: &[u16]) -> Option<SocketAddr> {
    let port = *reflector_ports.first()?;
    let without_scheme = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))?;
    let host_and_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    let host = host_and_port.split(':').next().unwrap_or(host_and_port);
    tokio::net::lookup_host((host, port)).await.ok()?.next()
}

/// Desktop-server transcode settings. The usable streaming budget is
/// `max_upload * (1 - reserve_percent)`; every negotiated playback session
/// reserves from that one aggregate pool.
pub fn transcode_config_from_env(data_dir: &std::path::Path) -> TranscodeConfig {
    let max_upload_mbps = std::env::var("SWARM_MAX_UPLOAD_MBPS")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(10.0);
    let reserve_percent = std::env::var("SWARM_UPLOAD_RESERVE_PERCENT")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(90)
        .min(90);
    let max_sessions = std::env::var("SWARM_MAX_STREAMS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(2);
    let disabled = std::env::var("SWARM_TRANSCODING_DISABLED")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false);
    TranscodeConfig {
        enabled: !disabled,
        ffmpeg_path: PathBuf::from(
            std::env::var("SWARM_FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into()),
        ),
        session_dir: data_dir.join("transcodes"),
        max_upload_bps: (max_upload_mbps * 1_000_000.0) as u64,
        reserve_percent,
        max_sessions,
        idle_timeout: std::time::Duration::from_secs(300),
        segment_duration_secs: 4,
    }
}
