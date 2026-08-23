//! Tauri desktop shell for the SWARM media server (feature `gui`).
//!
//! Thin windowing over [`swarm_server::ServerCore`]: the webview UI (in
//! `ui/`) calls the commands below via `window.__TAURI__.core.invoke`.
//! First-run onboarding picks a media folder (persisted to
//! `<app data dir>/settings.json`) before a core can start; joining a swarm
//! is a separate, skippable step reachable again later from the Swarm card.
//! Adding/removing a media root after the core has started takes effect
//! immediately (see `AppState::apply_live_roots` and
//! `ServerCore::update_media_roots`) — the QUIC listener itself is never
//! torn down, only the shared `SharedRootResolver` both the core and its
//! `MediaService` hold is swapped and a scan run against the new set.

mod mcp;
mod settings;

use rand::RngCore;
use settings::{MediaRootHealth, MediaRootSetting, Settings};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use swarm_core::peer::MediaKind;
use swarm_media::roots::MediaRoot;
use swarm_media::scrape::{BulkScrapeReport, ScrapeConfig};
use swarm_server::{ServerConfig, ServerCore, ServerStatus, TokenStoreMode};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::OnceCell;

struct AppState {
    core: OnceCell<Arc<ServerCore>>,
}

fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

fn configured_rendezvous_url() -> Option<String> {
    std::env::var("SWARM_RENDEZVOUS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            option_env!("SWARM_RENDEZVOUS_URL")
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty())
        })
}

impl AppState {
    /// Build the core from persisted settings on first use. Fails with the
    /// sentinel `"not_configured"` when no media folder has been chosen yet
    /// — the frontend checks for that exact string to show onboarding
    /// instead of a raw error.
    async fn core(&self, app: &tauri::AppHandle) -> Result<Arc<ServerCore>, String> {
        self.core
            .get_or_try_init(|| async {
                let dir = app_data_dir(app)?;
                let mut settings = settings::load(&dir);
                if settings.media_roots.is_empty() {
                    return Err("not_configured".to_string());
                }
                if settings::populate_reconnect_urls(&mut settings) {
                    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
                }
                let recovery_settings_dir = dir.clone();
                let config = ServerConfig {
                    media_roots: to_media_roots(&settings.media_roots),
                    data_dir: dir,
                    // This remains an environment override for development and
                    // managed deployments; ordinary desktop users never need it.
                    bind: std::env::var("SWARM_PEER_BIND")
                        .unwrap_or_else(|_| "0.0.0.0:8543".into())
                        .parse()
                        .expect("SWARM_PEER_BIND must be host:port"),
                    allowed_fingerprints: vec![],
                    // Real bug, found live: with PreferKeyring, a token saved
                    // successfully via the OS keychain has no file backup —
                    // `TokenStore::save` only falls back to the file when the
                    // keyring *write* itself fails. macOS Keychain access is
                    // tied to the app binary's code signature, so an
                    // unsigned/ad-hoc-resigned build (this app's normal dev
                    // state) can lose access to its own previously-saved
                    // entry after a rebuild; `restore_stun_link` then finds
                    // no token and discards the whole STUN link — even though
                    // base_url/device_id/swarms all survived fine in
                    // server-state.sqlite — forcing full STUN URL + join code
                    // re-entry. FileOnly's plain 0600 file isn't tied to code
                    // signing, so it survives every rebuild/restart; the
                    // token itself is already revocable-not-precious (the
                    // STUN server's own row is the real revocation
                    // authority), so this is the right trade-off here.
                    token_store_mode: TokenStoreMode::FileOnly,
                    managed_rendezvous_url: configured_rendezvous_url(),
                };
                let core = ServerCore::start(config).await.map_err(|e| e.to_string())?;
                core.set_streaming_upload_budget_enabled(settings.streaming_upload_budget_enabled);
                core.set_local_transcription_enabled(settings.local_transcription_enabled);
                core.set_transcription_pause_while_streaming(settings.transcription_pause_while_streaming);
                start_media_root_recovery(Arc::clone(&core), recovery_settings_dir);
                if settings.mcp_enabled {
                    if let Some(access_token) = settings.mcp_access_token.filter(|token| !token.is_empty()) {
                        let mcp_core = Arc::clone(&core);
                        tokio::spawn(async move {
                            if let Err(err) = mcp::serve(mcp_core, settings.mcp_port, access_token).await {
                                tracing::error!(%err, "MCP server stopped");
                            }
                        });
                    } else {
                        tracing::error!("MCP server is enabled but has no access token; create one in the AI tab");
                    }
                }
                Ok(core)
            })
            .await
            .cloned()
    }

    /// If a core is already running, live-apply a fresh root list (see
    /// `ServerCore::update_media_roots`) — `add_media_root`/`remove_media_root`
    /// take effect immediately instead of on next launch. A no-op, not an
    /// error, when no core exists yet (e.g. mid first-run onboarding, before
    /// any folder has ever been chosen): `OnceCell::get` never triggers
    /// initialization, so onboarding is unaffected.
    async fn apply_live_roots(
        &self,
        roots: &[MediaRootSetting],
    ) -> Result<Option<RescanResult>, String> {
        let Some(core) = self.core.get() else {
            return Ok(None);
        };
        let report = core
            .update_media_roots(to_media_roots(roots))
            .await
            .map_err(|e| e.to_string())?;
        Ok(Some(RescanResult {
            added: report.added,
            updated: report.updated,
            removed: report.removed,
            unchanged: report.unchanged,
        }))
    }
}

/// Network mounts can remain present under `/Volumes` while every read
/// fails. Poll the same real-read health check used by the UI, ask macOS to
/// remount known SMB roots with saved credentials, and rescan once after a
/// root becomes readable again. Attempts are throttled so an offline NAS
/// cannot produce a reconnect storm.
fn start_media_root_recovery(core: Arc<ServerCore>, settings_dir: PathBuf) {
    tokio::spawn(async move {
        let mut unavailable = HashSet::<String>::new();
        let mut last_attempt = HashMap::<String, Instant>::new();
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            let mut persisted = settings::load(&settings_dir);
            if settings::populate_reconnect_urls(&mut persisted) {
                if let Err(error) = settings::save(&settings_dir, &persisted) {
                    tracing::warn!(%error, "could not save detected network-share reconnect URL");
                }
            }
            let roots = persisted.media_roots;
            let health = tokio::task::spawn_blocking({
                let roots = roots.clone();
                move || settings::media_root_health(&roots)
            })
            .await
            .unwrap_or_default();
            let mut recovered = false;
            for (root, status) in roots.iter().zip(health) {
                if status.available {
                    if unavailable.remove(&root.label) {
                        tracing::info!(root = %root.label, path = %root.path, "media root recovered");
                        recovered = true;
                    }
                    last_attempt.remove(&root.label);
                    continue;
                }
                unavailable.insert(root.label.clone());
                let should_attempt = root.reconnect_url.is_some()
                    && last_attempt
                        .get(&root.label)
                        .is_none_or(|last| last.elapsed() >= Duration::from_secs(30));
                if should_attempt {
                    last_attempt.insert(root.label.clone(), Instant::now());
                    let reconnect_root = root.clone();
                    tokio::task::spawn_blocking(move || {
                        match settings::reconnect_network_root(&reconnect_root) {
                            Ok(true) => {
                                tracing::info!(root = %reconnect_root.label, "requested network-share reconnect")
                            }
                            Ok(false) => {}
                            Err(error) => {
                                tracing::warn!(root = %reconnect_root.label, %error, "network-share reconnect request failed")
                            }
                        }
                    });
                }
            }
            if recovered {
                if let Err(error) = core.rescan(None).await {
                    tracing::warn!(%error, "media-root recovery rescan failed");
                }
            }
        }
    });
}

fn to_media_roots(settings: &[MediaRootSetting]) -> Vec<MediaRoot> {
    settings
        .iter()
        .map(|r| MediaRoot {
            label: r.label.clone(),
            path: PathBuf::from(&r.path),
        })
        .collect()
}

#[derive(serde::Serialize)]
struct SettingsView {
    media_roots: Vec<MediaRootSetting>,
    has_tmdb_key: bool,
    has_opensubtitles_key: bool,
    streaming_upload_budget_enabled: bool,
    local_transcription_enabled: bool,
    transcription_pause_while_streaming: bool,
    mcp_enabled: bool,
    mcp_port: u16,
    mcp_access_token: Option<String>,
}

#[tauri::command]
async fn get_settings(app: tauri::AppHandle) -> Result<SettingsView, String> {
    let settings = settings::load(&app_data_dir(&app)?);
    Ok(SettingsView {
        media_roots: settings.media_roots,
        has_tmdb_key: settings.tmdb_api_key.is_some(),
        has_opensubtitles_key: settings.opensubtitles_api_key.is_some(),
        streaming_upload_budget_enabled: settings.streaming_upload_budget_enabled,
        local_transcription_enabled: settings.local_transcription_enabled,
        transcription_pause_while_streaming: settings.transcription_pause_while_streaming,
        mcp_enabled: settings.mcp_enabled,
        mcp_port: settings.mcp_port,
        mcp_access_token: settings.mcp_access_token,
    })
}

/// Does not initialize `ServerCore`, so the warning can render even when a
/// disconnected file share is the very thing preventing normal dashboard
/// work from completing.
#[tauri::command]
async fn get_media_root_health(app: tauri::AppHandle) -> Result<Vec<MediaRootHealth>, String> {
    let roots = settings::load(&app_data_dir(&app)?).media_roots;
    tokio::task::spawn_blocking(move || settings::media_root_health(&roots))
        .await
        .map_err(|error| error.to_string())
}

async fn pick_folder(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    Ok(rx.await.map_err(|e| e.to_string())?.map(|p| p.to_string()))
}

/// Opens the native folder picker and persists the choice as the app's
/// *first* media root, labeled `"local"` — the first-run onboarding path.
/// Does not affect an already-running core — see the module docs.
#[tauri::command]
async fn choose_media_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let Some(path) = pick_folder(&app).await? else {
        return Ok(None);
    };
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    settings.media_roots = vec![MediaRootSetting {
        label: "local".to_string(),
        path: path.clone(),
        reconnect_url: settings::discover_reconnect_url(&path),
    }];
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    Ok(Some(path))
}

/// Same native folder picker as [`choose_media_folder`], but only returns
/// the chosen path — no persistence. Used by the "add another root" flow
/// (Details tab), which needs the user to also supply a label before
/// `add_media_root` actually saves anything.
#[tauri::command]
async fn pick_folder_path(app: tauri::AppHandle) -> Result<Option<String>, String> {
    pick_folder(&app).await
}

/// Native file picker, filtered to common image types — for the "upload
/// artwork" flow (Media tab), paired with [`read_file_bytes`].
#[tauri::command]
async fn pick_file_path(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Images", &["jpg", "jpeg", "png", "webp"])
        .pick_file(move |picked| {
            let _ = tx.send(picked);
        });
    Ok(rx.await.map_err(|e| e.to_string())?.map(|p| p.to_string()))
}

/// Reads a file's raw bytes for upload — paired with [`pick_file_path`],
/// which is how the frontend obtains a path it's allowed to read (the
/// webview otherwise has no filesystem access of its own).
#[tauri::command]
async fn read_file_bytes(path: String) -> Result<Vec<u8>, String> {
    tokio::fs::read(&path).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_media_roots(app: tauri::AppHandle) -> Result<Vec<MediaRootSetting>, String> {
    Ok(settings::load(&app_data_dir(&app)?).media_roots)
}

#[derive(serde::Serialize)]
struct MediaRootsResult {
    media_roots: Vec<MediaRootSetting>,
    /// Present when a core was already running and the change was applied
    /// live; absent during first-run onboarding, before any core exists.
    rescan: Option<RescanResult>,
}

/// Adds an additional named root (e.g. a mounted NAS share) alongside
/// whatever's already configured. Applied live to an already-running core —
/// see the module docs.
#[tauri::command]
async fn add_media_root(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    label: String,
    path: String,
) -> Result<MediaRootsResult, String> {
    let label = label.trim().to_string();
    let path = path.trim().to_string();
    if label.is_empty() || path.is_empty() {
        return Err("label and path are both required".to_string());
    }
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    if settings.media_roots.iter().any(|r| r.label == label) {
        return Err(format!("a root labeled \"{label}\" already exists"));
    }
    let reconnect_url = settings::discover_reconnect_url(&path);
    settings.media_roots.push(MediaRootSetting {
        label,
        path,
        reconnect_url,
    });
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    let rescan = state.apply_live_roots(&settings.media_roots).await?;
    Ok(MediaRootsResult {
        media_roots: settings.media_roots,
        rescan,
    })
}

/// Removes a configured root by label. Refuses to remove the last remaining
/// root — a server always needs at least one. Applied live to an
/// already-running core — see the module docs.
#[tauri::command]
async fn remove_media_root(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    label: String,
) -> Result<MediaRootsResult, String> {
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    if settings.media_roots.len() <= 1 {
        return Err("at least one media root is required".to_string());
    }
    settings.media_roots.retain(|r| r.label != label);
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    let rescan = state.apply_live_roots(&settings.media_roots).await?;
    Ok(MediaRootsResult {
        media_roots: settings.media_roots,
        rescan,
    })
}

#[tauri::command]
async fn set_tmdb_api_key(app: tauri::AppHandle, key: String) -> Result<(), String> {
    let dir = app_data_dir(&app)?;
    let mut settings: Settings = settings::load(&dir);
    settings.tmdb_api_key = if key.trim().is_empty() {
        None
    } else {
        Some(key.trim().to_string())
    };
    settings::save(&dir, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_opensubtitles_api_key(app: tauri::AppHandle, key: String) -> Result<(), String> {
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    settings.opensubtitles_api_key = if key.trim().is_empty() {
        None
    } else {
        Some(key.trim().to_string())
    };
    settings::save(&dir, &settings).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct SubtitleDownloadResult {
    language: String,
    label: String,
}

#[tauri::command]
async fn download_subtitle(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    entry_key: String,
    language: String,
) -> Result<SubtitleDownloadResult, String> {
    let settings = settings::load(&app_data_dir(&app)?);
    let api_key = settings
        .opensubtitles_api_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| "Add an OpenSubtitles API key in Details first.".to_string())?;
    let track = state
        .core(&app)
        .await?
        .download_subtitle(api_key, &entry_key, &language)
        .await?;
    Ok(SubtitleDownloadResult {
        language: track.language,
        label: track.label,
    })
}

#[tauri::command]
async fn set_streaming_upload_budget_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    settings.streaming_upload_budget_enabled = enabled;
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    if let Some(core) = state.core.get() {
        core.set_streaming_upload_budget_enabled(enabled);
    }
    Ok(())
}

#[tauri::command]
async fn set_local_transcription_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    settings.local_transcription_enabled = enabled;
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    let core = state.core(&app).await?;
    core.set_local_transcription_enabled(enabled);
    Ok(())
}

#[tauri::command]
async fn set_transcription_pause_while_streaming(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    settings.transcription_pause_while_streaming = enabled;
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    let core = state.core(&app).await?;
    core.set_transcription_pause_while_streaming(enabled);
    Ok(())
}

#[tauri::command]
async fn get_transcription_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<swarm_server::transcription::TranscriptionStatus, String> {
    state
        .core(&app)
        .await?
        .transcription_status()
        .await
        .map_err(|e| e.to_string())
}

/// Both take effect on next launch/restart, not live — see `mcp.rs`'s doc
/// comment and `AppState::core`, which only ever starts the MCP listener
/// once, the same time it starts `ServerCore` itself.
#[tauri::command]
async fn set_mcp_enabled(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let dir = app_data_dir(&app)?;
    let mut settings: Settings = settings::load(&dir);
    settings.mcp_enabled = enabled;
    settings::save(&dir, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_mcp_port(app: tauri::AppHandle, port: u16) -> Result<(), String> {
    let dir = app_data_dir(&app)?;
    let mut settings: Settings = settings::load(&dir);
    settings.mcp_port = port;
    settings::save(&dir, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
async fn generate_mcp_access_token(app: tauri::AppHandle) -> Result<String, String> {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = format!("swarm_mcp_{}", hex::encode(bytes));
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    settings.mcp_access_token = Some(token.clone());
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    Ok(token)
}

#[tauri::command]
async fn get_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ServerStatus, String> {
    state
        .core(&app)
        .await?
        .status()
        .await
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct RescanResult {
    added: u64,
    updated: u64,
    removed: u64,
    unchanged: u64,
}

/// `scan-progress` event name emitted to the webview during [`rescan`] — one
/// per discovered file, payload shape is `swarm_media::scan::ScanProgressEvent`.
/// Same forwarding-task pattern as [`run_scrape`]'s `scrape-progress` — real
/// bug this fixes: a rescan over a slow network mount gave no indication
/// anything was happening until it finished, confirmed live against a real
/// ~3,700-file remote share.
const SCAN_PROGRESS_EVENT: &str = "scan-progress";

#[tauri::command]
async fn rescan(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<RescanResult, String> {
    let core = state.core(&app).await?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let emitter = app.clone();
    let forward = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = emitter.emit(SCAN_PROGRESS_EVENT, event);
        }
    });
    let result = core.rescan(Some(tx)).await.map_err(|e| e.to_string());
    let _ = forward.await;
    let report = result?;
    Ok(RescanResult {
        added: report.added,
        updated: report.updated,
        removed: report.removed,
        unchanged: report.unchanged,
    })
}

/// Re-derives every entry's classification from its already-stored path —
/// repairs entries a `classify()` bug already misfiled (wrong kind/show/
/// season/episode) without needing the underlying file to change, which a
/// plain Rescan can't do (it only re-classifies new/modified files — see
/// `Library::reclassify_all`'s doc comment). Clears stale scrape data for
/// whatever it actually corrects; run a normal Scrape metadata afterward to
/// pick those back up under their now-correct classification.
#[tauri::command]
async fn reclassify_library(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<swarm_media::store::ReclassifyReport, String> {
    let core = state.core(&app).await?;
    core.library
        .reclassify_all(&core.media_roots)
        .await
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct EntrySummary {
    entry_key: String,
    kind: String,
    title: String,
    relative_path: String,
    size: u64,
    scraped_title: Option<String>,
    genres: Vec<String>,
    has_artwork: bool,
    // Grouping/detail fields for the Media tab's hierarchical browse view
    // (artist/album/track for music, show/season/episode for TV) — see
    // `apps/server/ui/media.js`'s `groupTracks`/`groupEpisodes`. All
    // path-derived (never scraper output), per `classify.rs`'s grouping-key
    // invariant.
    artist: Option<String>,
    album: Option<String>,
    track_number: Option<u32>,
    show_title: Option<String>,
    season: Option<u32>,
    episode: Option<u32>,
    year: Option<u32>,
    duration_secs: Option<f64>,
    cast: Vec<swarm_media::store::CastMember>,
    overview: Option<String>,
    rating: Option<String>,
    community_rating: Option<f64>,
    community_rating_votes: Option<u64>,
    like_count: u32,
}

#[tauri::command]
async fn list_entries(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<EntrySummary>, String> {
    let core = state.core(&app).await?;
    let entries = core.library.list().await.map_err(|e| e.to_string())?;
    let like_counts = core
        .library
        .like_counts()
        .await
        .map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .map(|entry| EntrySummary {
            like_count: like_counts.get(&entry.entry_key).copied().unwrap_or(0),
            entry_key: entry.entry_key,
            kind: format!("{:?}", entry.kind).to_lowercase(),
            title: entry.title,
            relative_path: entry.relative_path,
            size: entry.size,
            scraped_title: entry.scraped_title,
            genres: entry.genres,
            has_artwork: entry.artwork_version > 0,
            artist: entry.artist,
            album: entry.album,
            track_number: entry.track_number,
            show_title: entry.show_title,
            season: entry.season,
            episode: entry.episode,
            year: entry.year,
            duration_secs: entry.duration_secs,
            cast: entry.cast,
            overview: entry.overview,
            rating: entry.rating,
            community_rating: entry.community_rating,
            community_rating_votes: entry.community_rating_votes,
        })
        .collect())
}

/// Every distinct genre/category value currently in use anywhere in the
/// library — backs the Media tab's category picker, see
/// `Library::distinct_genres`'s doc comment for why genres double as
/// categories rather than this being a separate concept.
#[tauri::command]
async fn list_categories(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let core = state.core(&app).await?;
    core.library
        .distinct_genres()
        .await
        .map_err(|e| e.to_string())
}

/// Raw bytes for one entry's artwork slot, for the Media tab's browse view to
/// render as an `<img>` — this GUI runs in the same process as the media
/// server but `/art/{entry_key}/{kind}` is only reachable over the P2P QUIC
/// peer protocol (see `docs/PROTOCOL.md`), which a webview can't speak
/// directly, so this reads the file straight off disk instead. `Ok(None)`
/// means no artwork of that kind was ever scraped/uploaded — not an error.
#[tauri::command]
async fn get_artwork_bytes(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    entry_key: String,
    kind: String,
) -> Result<Option<Vec<u8>>, String> {
    let core = state.core(&app).await?;
    let artwork_kind = swarm_media::store::ArtworkKind::parse(&kind)
        .ok_or_else(|| format!("unknown artwork kind \"{kind}\""))?;
    let Some((relative_path, _version)) = core
        .library
        .artwork(&entry_key, artwork_kind)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let path = core.media_roots.resolve(&relative_path);
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// `scrape-progress` event name emitted to the webview during
/// [`run_scrape`] — one per entry, payload shape is `ScrapeProgressEvent`.
/// The frontend listens via `window.__TAURI__.event.listen`.
const SCRAPE_PROGRESS_EVENT: &str = "scrape-progress";

#[tauri::command]
async fn run_scrape(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    force: bool,
) -> Result<BulkScrapeReport, String> {
    let core = state.core(&app).await?;
    let tmdb_api_key = settings::load(&app_data_dir(&app)?).tmdb_api_key;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let emitter = app.clone();
    // Forwarding task: relays each progress event to the webview as it
    // arrives, independent of `run_scrape` itself, which is only awaited
    // for its final report below.
    let forward = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = emitter.emit(SCRAPE_PROGRESS_EVENT, event);
        }
    });
    let result = core
        .run_scrape(
            ScrapeConfig {
                tmdb_api_key,
                ..Default::default()
            },
            Some(tx),
            force,
        )
        .await;
    // Dropping the last sender (above) closes the channel, so `forward`
    // exits its loop on its own — awaiting it here just makes sure every
    // already-queued event is actually emitted before this command returns
    // its final report, so the frontend can't see "done" before the last
    // per-entry update.
    let _ = forward.await;
    match &result {
        Ok(report) if !report.issues.is_empty() => {
            let mut message = format!(
                "Matched: {}\nNot found: {}\nFailed: {}\nSkipped: {}",
                report.matched, report.not_found, report.failed, report.skipped,
            );
            if !report.issues.is_empty() {
                message.push_str("\n\nIssues:\n");
                for issue in &report.issues {
                    message.push_str(&format!("• {} — {}\n", issue.title, issue.reason));
                }
            }
            let level = if report.failed > 0 {
                "error"
            } else {
                "warning"
            };
            if let Err(error) = core
                .library
                .record_server_notification(
                    level,
                    "Metadata scrape finished with issues",
                    message.trim_end(),
                )
                .await
            {
                tracing::warn!(%error, "could not save bulk-scrape error notification");
            }
        }
        Err(error) => {
            if let Err(save_error) = core
                .library
                .record_server_notification("error", "Metadata scrape failed", &error.to_string())
                .await
            {
                tracing::warn!(%save_error, "could not save bulk-scrape failure notification");
            }
        }
        _ => {}
    }
    result.map_err(|e| e.to_string())
}

/// Pinpoint rescrape of one entry, optionally against a manual TMDb id/URL
/// override (music entries ignore `tmdb_url` — no TMDb concept there).
#[tauri::command]
async fn rescrape_entry(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    entry_key: String,
    tmdb_url: Option<String>,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    let tmdb_api_key = settings::load(&app_data_dir(&app)?).tmdb_api_key;
    let config = ScrapeConfig {
        tmdb_api_key,
        ..Default::default()
    };
    let tmdb_override = tmdb_url
        .filter(|u| !u.trim().is_empty())
        .map(swarm_media::scrape::TmdbOverride::Url);
    core.rescrape_entry(&entry_key, config, tmdb_override)
        .await
        .map_err(|e| e.to_string())
}

/// Manually override an entry's display title, genre/category list,
/// synopsis, and/or content rating. `None` (omitted from the JS call) leaves
/// that field untouched — see `Library::set_manual_metadata`/
/// `Library::set_overview`/`Library::set_rating`. Never affects grouping
/// (artist/album/show/season/episode), which stays path-derived.
#[tauri::command]
async fn set_manual_metadata(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    entry_key: String,
    title: Option<String>,
    genres: Option<Vec<String>>,
    overview: Option<String>,
    rating: Option<String>,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    core.library
        .set_manual_metadata(&entry_key, title.as_deref(), genres.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    if let Some(overview) = overview {
        core.library
            .set_overview(&entry_key, &overview)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(rating) = rating {
        core.library
            .set_rating(&entry_key, &rating)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Manually move an entry to a different asset type ("movie"/"episode"/
/// "track") — see `Library::set_manual_kind`'s doc comment for why this
/// exists (a music video sitting under `movies/` or `shows/` as an .mkv,
/// indistinguishable from a real movie/episode by path or extension alone).
/// `artist`/`album` matter only when `kind` is "track"; `show_title` only
/// when it's "episode" — both ignored otherwise.
#[tauri::command]
async fn set_manual_kind(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    entry_key: String,
    kind: String,
    artist: Option<String>,
    album: Option<String>,
    show_title: Option<String>,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    let kind = match kind.as_str() {
        "movie" => MediaKind::Movie,
        "episode" => MediaKind::Episode,
        "track" => MediaKind::Track,
        other => return Err(format!("unknown asset kind \"{other}\"")),
    };
    core.library
        .set_manual_kind(
            &entry_key,
            kind,
            artist.as_deref(),
            album.as_deref(),
            show_title.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())
}

/// Manually uploaded artwork bytes for one entry/kind — the same on-disk
/// convention (`images/` sibling folder) and `Library::set_artwork` call the
/// scraper itself uses, just sourced from a file picker instead of a
/// download. `extension` (no leading dot, e.g. `"png"`) comes from whatever
/// file the user picked, since manually uploaded art isn't always a jpg like
/// every scraped image is today.
#[tauri::command]
async fn upload_artwork(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    entry_key: String,
    kind: String,
    extension: String,
    bytes: Vec<u8>,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    let artwork_kind = swarm_media::store::ArtworkKind::parse(&kind)
        .ok_or_else(|| format!("unknown artwork kind \"{kind}\""))?;
    let extension = extension.trim().trim_start_matches('.').to_lowercase();
    if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        return Err(format!("unsupported image extension \"{extension}\""));
    }
    let entry = core
        .library
        .get(&entry_key)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no such entry")?;
    let filename = format!("manual-{}.{extension}", artwork_kind.route_segment());
    let relative = swarm_media::scrape::artwork::save_artwork(
        &core.media_roots,
        &entry.relative_path,
        &filename,
        &bytes,
    )
    .await
    .map_err(|e| e.to_string())?;
    core.library
        .set_artwork(&entry_key, artwork_kind, &relative)
        .await
        .map_err(|e| e.to_string())
}

/// Manually uploaded artwork shared across a whole client-side-computed
/// group — an artist (every track by that artist, across every album) or an
/// album (every track in it) — rather than one entry, since neither has an
/// `entry_key` of its own (the Netflix-style hierarchy is grouped entirely
/// client-side over the flat entry list; see the plan's "hierarchy is
/// grouped client-side" decision). The frontend already knows exactly which
/// entries belong to the group (it just built the grouping to render the
/// page), so it passes their keys directly rather than this command
/// re-deriving artist/album matches itself. Same on-disk convention as
/// [upload_artwork] and the scraper's own [`crate`]-external
/// `scrape_one_album_group` (`swarm_media::scrape::runner`): the file is
/// saved once, beside the *first* entry, and every entry in the group
/// stores that same shared `relative_path` — safe regardless of whether the
/// group's tracks span multiple folders, since artwork is always resolved
/// root-relative (`get_artwork_bytes` above), never relative to the
/// referencing entry's own folder.
#[tauri::command]
async fn upload_group_artwork(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    entry_keys: Vec<String>,
    kind: String,
    extension: String,
    bytes: Vec<u8>,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    let artwork_kind = swarm_media::store::ArtworkKind::parse(&kind)
        .ok_or_else(|| format!("unknown artwork kind \"{kind}\""))?;
    let extension = extension.trim().trim_start_matches('.').to_lowercase();
    if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        return Err(format!("unsupported image extension \"{extension}\""));
    }
    let Some(first_key) = entry_keys.first() else {
        return Err("no entries to attach artwork to".to_string());
    };
    let first = core
        .library
        .get(first_key)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no such entry")?;
    let filename = format!("manual-{}.{extension}", artwork_kind.route_segment());
    let relative = swarm_media::scrape::artwork::save_artwork(
        &core.media_roots,
        &first.relative_path,
        &filename,
        &bytes,
    )
    .await
    .map_err(|e| e.to_string())?;
    for entry_key in &entry_keys {
        core.library
            .set_artwork(entry_key, artwork_kind, &relative)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Reverts a bad scrape (wrong TMDb/MusicBrainz match, bad manual edit) —
/// clears the scraped title/genres/cast and every artwork slot, and puts the
/// entry back into `missing_scrape` so a plain (non-force) bulk scrape or a
/// pinpoint rescrape will pick it up fresh. Also best-effort deletes the
/// now-orphaned artwork files from disk; a deletion failure (e.g. the file
/// was already gone, or a flaky network mount) never fails the command
/// itself — the database state is what actually matters here.
#[tauri::command]
async fn clear_scraped_metadata(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    entry_key: String,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    let cleared_paths = core
        .library
        .clear_scrape_result(&entry_key)
        .await
        .map_err(|e| e.to_string())?;
    for relative_path in cleared_paths {
        let path = core.media_roots.resolve(&relative_path);
        let _ = tokio::fs::remove_file(&path).await;
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct SwarmSummaryView {
    id: String,
    name: String,
}

#[derive(serde::Serialize)]
struct SwarmLinkView {
    base_url: String,
    swarms: Vec<SwarmSummaryView>,
    allowed_peer_count: usize,
}

#[tauri::command]
async fn get_swarm_link(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<SwarmLinkView>, String> {
    let core = state.core(&app).await?;
    let Some(link) = core.stun_link().await else {
        return Ok(None);
    };
    Ok(Some(SwarmLinkView {
        base_url: link.base_url,
        swarms: link
            .swarms
            .into_iter()
            .map(|s| SwarmSummaryView {
                id: s.id,
                name: s.name,
            })
            .collect(),
        allowed_peer_count: core.allowed.len(),
    }))
}

/// Accepts the short-lived code displayed by a TV discovered on the LAN.
/// This approval path is entirely local and independent of the SWARM service.
#[tauri::command]
async fn approve_lan_pairing(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    code: String,
) -> Result<swarm_server::lan::LanPairingApproval, String> {
    let core = state.core(&app).await?;
    core.approve_lan_pairing(&code)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn list_local_peers(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<swarm_server::LocalPeerRecord>, String> {
    let core = state.core(&app).await?;
    core.local_peers().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn revoke_local_peer(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    fingerprint: String,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    core.revoke_local_peer(&fingerprint)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn join_swarm(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    base_url: String,
    code: String,
    device_name: String,
) -> Result<SwarmSummaryView, String> {
    let core = state.core(&app).await?;
    let swarm = core
        .register_with_stun(&base_url, &code, &device_name)
        .await
        .map_err(|e| e.to_string())?;
    Ok(SwarmSummaryView {
        id: swarm.id,
        name: swarm.name,
    })
}

#[tauri::command]
async fn join_additional_swarm(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    code: String,
) -> Result<SwarmSummaryView, String> {
    let core = state.core(&app).await?;
    let swarm = core
        .join_additional_swarm(&code)
        .await
        .map_err(|e| e.to_string())?;
    Ok(SwarmSummaryView {
        id: swarm.id,
        name: swarm.name,
    })
}

#[tauri::command]
async fn lookup_tv_activation(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    code: String,
) -> Result<swarm_core::rest::ActivationPreview, String> {
    let core = state.core(&app).await?;
    core.lookup_activation(&code)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn approve_tv_activation(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    activation_id: String,
) -> Result<swarm_core::rest::ActivationStatusResponse, String> {
    let core = state.core(&app).await?;
    core.approve_activation(&activation_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn resync_swarm(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<usize, String> {
    let core = state.core(&app).await?;
    core.resync().await.map_err(|e| e.to_string())
}

/// Leave one joined swarm, keeping the STUN link (and other memberships)
/// intact — see `ServerCore::leave_swarm`.
#[tauri::command]
async fn leave_swarm(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    swarm_id: String,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    core.leave_swarm(&swarm_id).await.map_err(|e| e.to_string())
}

/// One joined swarm's full device roster (type, online status, fingerprint,
/// last-seen, free-form metadata) — see `ServerCore::swarm_devices`. Passcode
/// *generation* isn't exposed here: the STUN server only allows the swarm's
/// owning user (a session-cookie browser login) to mint join codes, and this
/// device only ever holds a Bearer access token — that's a real auth-model
/// gap, not an oversight, and codes are generated from the STUN server's own
/// admin page today (see `get_swarm_link`'s `base_url`).
#[tauri::command]
async fn get_swarm_devices(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    swarm_id: String,
) -> Result<swarm_core::rest::SwarmDevicesResponse, String> {
    let core = state.core(&app).await?;
    core.swarm_devices(&swarm_id)
        .await
        .map_err(|e| e.to_string())
}

/// Wire shape for the swarm page's Errors panel — a plain struct rather than
/// reusing `swarm_media::store::ClientErrorRecord` directly so the frontend
/// gets stable camelCase field names independent of the Rust struct's own
/// naming, same pattern as `EntrySummary` above.
#[derive(serde::Serialize)]
struct ClientErrorView {
    id: i64,
    device_id: String,
    device_name: String,
    entry_key: Option<String>,
    asset_title: Option<String>,
    kind: Option<String>,
    message: String,
    context: Option<String>,
    occurred_at_ms: i64,
    received_at_ms: i64,
}

impl From<swarm_media::store::ClientErrorRecord> for ClientErrorView {
    fn from(r: swarm_media::store::ClientErrorRecord) -> Self {
        Self {
            id: r.id,
            device_id: r.device_id,
            device_name: r.device_name,
            entry_key: r.entry_key,
            asset_title: r.asset_title,
            kind: r.kind,
            message: r.message,
            context: r.context,
            occurred_at_ms: r.occurred_at_ms,
            received_at_ms: r.received_at_ms,
        }
    }
}

/// Client-reported errors (playback failures, etc. — see
/// `swarm_core::peer::ClientErrorReport`), newest first, for the swarm
/// page's Errors panel.
#[tauri::command]
async fn list_client_errors(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ClientErrorView>, String> {
    let core = state.core(&app).await?;
    let errors = core
        .library
        .list_client_errors()
        .await
        .map_err(|e| e.to_string())?;
    Ok(errors.into_iter().map(ClientErrorView::from).collect())
}

/// Backs the swarm page's notification bubble — polled separately from
/// [`list_client_errors`] so the badge can refresh cheaply without pulling
/// every error's full body down each time.
#[tauri::command]
async fn client_error_count(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<u64, String> {
    let core = state.core(&app).await?;
    core.library
        .client_error_count()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_client_error(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    core.library
        .delete_client_error(id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn clear_client_errors(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    core.library
        .clear_client_errors()
        .await
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct ServerNotificationView {
    id: i64,
    level: String,
    title: String,
    message: String,
    created_at_ms: i64,
}

impl From<swarm_media::store::ServerNotificationRecord> for ServerNotificationView {
    fn from(notification: swarm_media::store::ServerNotificationRecord) -> Self {
        Self {
            id: notification.id,
            level: notification.level,
            title: notification.title,
            message: notification.message,
            created_at_ms: notification.created_at_ms,
        }
    }
}

#[tauri::command]
async fn list_server_notifications(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ServerNotificationView>, String> {
    let core = state.core(&app).await?;
    core.library
        .list_server_notifications()
        .await
        .map(|notifications| {
            notifications
                .into_iter()
                .map(ServerNotificationView::from)
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn notification_count(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<u64, String> {
    let core = state.core(&app).await?;
    let server = core
        .library
        .server_notification_count()
        .await
        .map_err(|error| error.to_string())?;
    let client = core
        .library
        .client_error_count()
        .await
        .map_err(|error| error.to_string())?;
    Ok(server + client)
}

#[tauri::command]
async fn delete_server_notification(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    core.library
        .delete_server_notification(id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn clear_server_notifications(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    core.library
        .clear_server_notifications()
        .await
        .map_err(|error| error.to_string())
}

// The dashboard's info modal (apps/server/ui/app.js's INFO_TOPICS) links out to
// external resources (protocol/standard explainers) for the curious — a plain
// `<a target="_blank">` doesn't open the OS's default browser from inside a
// Tauri webview the way it would in a real browser tab; it's either silently
// swallowed or, at best, opens a second app window pointed at an external
// site, neither of which is what "learn more" should do. The opener plugin's
// `open_url` is the actual supported way to hand a URL to the OS. Called from
// a plain app command (not invoked as `plugin:opener|open_url` directly from
// JS) so no extra capability/permission entry is needed — same reasoning
// `choose_media_folder` below wraps the dialog plugin instead of exposing it
// to JS directly.
#[tauri::command]
fn open_external_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

const MAIN_WINDOW_LABEL: &str = "main";

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Hiding the desktop window deliberately leaves [`AppState`] and its
/// [`ServerCore`] alive. Playback, LAN discovery, and remote connections keep
/// running until the user chooses Quit from the tray menu.
#[tauri::command]
fn hide_to_tray(app: tauri::AppHandle) -> Result<(), String> {
    app.get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "main window is unavailable".to_string())?
        .hide()
        .map_err(|error| error.to_string())
}

fn install_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "tray-show", "Show SWARM", true, None::<&str>)?;
    let running = MenuItem::with_id(
        app,
        "tray-running",
        "Media server continues while hidden",
        false,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "tray-quit", "Quit SWARM", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &running, &separator, &quit])?;

    let mut tray = TrayIconBuilder::with_id("swarm-server")
        .menu(&menu)
        .tooltip("SWARM Media Server")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-show" => show_main_window(app),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    let app = tauri::Builder::default()
        // Must be registered first: a second launch simply reveals the
        // already-running window instead of starting a competing server.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            install_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == MAIN_WINDOW_LABEL {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .manage(AppState {
            core: OnceCell::new(),
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            get_media_root_health,
            hide_to_tray,
            open_external_url,
            choose_media_folder,
            pick_folder_path,
            pick_file_path,
            read_file_bytes,
            list_media_roots,
            add_media_root,
            remove_media_root,
            set_tmdb_api_key,
            set_opensubtitles_api_key,
            download_subtitle,
            set_streaming_upload_budget_enabled,
            set_local_transcription_enabled,
            set_transcription_pause_while_streaming,
            get_transcription_status,
            set_mcp_enabled,
            set_mcp_port,
            generate_mcp_access_token,
            get_status,
            rescan,
            reclassify_library,
            list_entries,
            list_categories,
            get_artwork_bytes,
            run_scrape,
            rescrape_entry,
            set_manual_metadata,
            set_manual_kind,
            upload_artwork,
            upload_group_artwork,
            clear_scraped_metadata,
            get_swarm_link,
            approve_lan_pairing,
            list_local_peers,
            revoke_local_peer,
            join_swarm,
            join_additional_swarm,
            lookup_tv_activation,
            approve_tv_activation,
            resync_swarm,
            leave_swarm,
            get_swarm_devices,
            list_client_errors,
            client_error_count,
            delete_client_error,
            clear_client_errors,
            list_server_notifications,
            notification_count,
            delete_server_notification,
            clear_server_notifications,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build SWARM Server");

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main_window(app);
        }
    });
}
