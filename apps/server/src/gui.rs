//! Tauri desktop shell for the SWARM media server (feature `gui`).
//!
//! Thin windowing over [`swarm_server::ServerCore`]: the webview UI (in
//! `ui/`) calls the commands below via `window.__TAURI__.core.invoke`.
//! First-run onboarding picks a media folder (persisted to
//! `<app data dir>/settings.json`) before a core can start; joining a swarm
//! is a separate, skippable step reachable again later from the Swarm card.
//! Changing the media folder after the core has started takes effect on the
//! next launch — tearing down a live QUIC listener mid-session isn't worth
//! the complexity for what is, in practice, a rare change.

mod settings;

use settings::{MediaRootSetting, Settings};
use std::path::PathBuf;
use std::sync::Arc;
use swarm_media::roots::MediaRoot;
use swarm_media::scrape::{BulkScrapeReport, ScrapeConfig};
use swarm_server::{ServerConfig, ServerCore, ServerStatus, TokenStoreMode};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use tokio::sync::OnceCell;

struct AppState {
    core: OnceCell<Arc<ServerCore>>,
}

fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
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
                let media_roots = settings::load(&dir).media_roots;
                if media_roots.is_empty() {
                    return Err("not_configured".to_string());
                }
                let config = ServerConfig {
                    media_roots: media_roots
                        .into_iter()
                        .map(|r| MediaRoot { label: r.label, path: PathBuf::from(r.path) })
                        .collect(),
                    data_dir: dir,
                    // Same SWARM_PEER_BIND convention as the headless binary
                    // (see config_from_env) — lets the GUI run on a different
                    // port than a headless instance for side-by-side testing,
                    // since both otherwise default to the identical 8543.
                    bind: std::env::var("SWARM_PEER_BIND")
                        .unwrap_or_else(|_| "0.0.0.0:8543".into())
                        .parse()
                        .expect("SWARM_PEER_BIND must be host:port"),
                    allowed_fingerprints: vec![],
                    token_store_mode: TokenStoreMode::PreferKeyring,
                };
                let (core, _report) = ServerCore::start(config).await.map_err(|e| e.to_string())?;
                Ok(core)
            })
            .await
            .cloned()
    }
}

#[derive(serde::Serialize)]
struct SettingsView {
    media_roots: Vec<MediaRootSetting>,
    has_tmdb_key: bool,
}

#[tauri::command]
async fn get_settings(app: tauri::AppHandle) -> Result<SettingsView, String> {
    let settings = settings::load(&app_data_dir(&app)?);
    Ok(SettingsView { media_roots: settings.media_roots, has_tmdb_key: settings.tmdb_api_key.is_some() })
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
    let Some(path) = pick_folder(&app).await? else { return Ok(None) };
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    settings.media_roots = vec![MediaRootSetting { label: "local".to_string(), path: path.clone() }];
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

/// Adds an additional named root (e.g. a mounted NAS share) alongside
/// whatever's already configured. Like `choose_media_folder`, takes effect
/// on next launch — see the module docs.
#[tauri::command]
async fn add_media_root(app: tauri::AppHandle, label: String, path: String) -> Result<Vec<MediaRootSetting>, String> {
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
    settings.media_roots.push(MediaRootSetting { label, path });
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    Ok(settings.media_roots)
}

/// Removes a configured root by label. Refuses to remove the last remaining
/// root — a server always needs at least one.
#[tauri::command]
async fn remove_media_root(app: tauri::AppHandle, label: String) -> Result<Vec<MediaRootSetting>, String> {
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    if settings.media_roots.len() <= 1 {
        return Err("at least one media root is required".to_string());
    }
    settings.media_roots.retain(|r| r.label != label);
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    Ok(settings.media_roots)
}

#[tauri::command]
async fn set_tmdb_api_key(app: tauri::AppHandle, key: String) -> Result<(), String> {
    let dir = app_data_dir(&app)?;
    let mut settings: Settings = settings::load(&dir);
    settings.tmdb_api_key = if key.trim().is_empty() { None } else { Some(key.trim().to_string()) };
    settings::save(&dir, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_status(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<ServerStatus, String> {
    state.core(&app).await?.status().await.map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct RescanResult {
    added: u64,
    updated: u64,
    removed: u64,
    unchanged: u64,
}

#[tauri::command]
async fn rescan(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<RescanResult, String> {
    let report = state.core(&app).await?.rescan().await.map_err(|e| e.to_string())?;
    Ok(RescanResult {
        added: report.added,
        updated: report.updated,
        removed: report.removed,
        unchanged: report.unchanged,
    })
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
}

#[tauri::command]
async fn list_entries(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<Vec<EntrySummary>, String> {
    let core = state.core(&app).await?;
    let entries = core.library.list().await.map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .map(|entry| EntrySummary {
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
        })
        .collect())
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
    let artwork_kind =
        swarm_media::store::ArtworkKind::parse(&kind).ok_or_else(|| format!("unknown artwork kind \"{kind}\""))?;
    let Some((relative_path, _version)) =
        core.library.artwork(&entry_key, artwork_kind).await.map_err(|e| e.to_string())?
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

#[tauri::command]
async fn run_scrape(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<BulkScrapeReport, String> {
    let core = state.core(&app).await?;
    let tmdb_api_key = settings::load(&app_data_dir(&app)?).tmdb_api_key;
    core.run_scrape(ScrapeConfig { tmdb_api_key, ..Default::default() }).await.map_err(|e| e.to_string())
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
    let config = ScrapeConfig { tmdb_api_key, ..Default::default() };
    let tmdb_override = tmdb_url.filter(|u| !u.trim().is_empty()).map(swarm_media::scrape::TmdbOverride::Url);
    core.rescrape_entry(&entry_key, config, tmdb_override).await.map_err(|e| e.to_string())
}

/// Manually override an entry's display title and/or genre list. `None`
/// (omitted from the JS call) leaves that field untouched — see
/// `Library::set_manual_metadata`. Never affects grouping (artist/album/
/// show/season/episode), which stays path-derived.
#[tauri::command]
async fn set_manual_metadata(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    entry_key: String,
    title: Option<String>,
    genres: Option<Vec<String>>,
) -> Result<(), String> {
    let core = state.core(&app).await?;
    core.library
        .set_manual_metadata(&entry_key, title.as_deref(), genres.as_deref())
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
    let artwork_kind =
        swarm_media::store::ArtworkKind::parse(&kind).ok_or_else(|| format!("unknown artwork kind \"{kind}\""))?;
    let extension = extension.trim().trim_start_matches('.').to_lowercase();
    if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        return Err(format!("unsupported image extension \"{extension}\""));
    }
    let entry = core.library.get(&entry_key).await.map_err(|e| e.to_string())?.ok_or("no such entry")?;
    let filename = format!("manual-{}.{extension}", artwork_kind.route_segment());
    let relative = swarm_media::scrape::artwork::save_artwork(&core.media_roots, &entry.relative_path, &filename, &bytes)
        .await
        .map_err(|e| e.to_string())?;
    core.library.set_artwork(&entry_key, artwork_kind, &relative).await.map_err(|e| e.to_string())
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
    let Some(link) = core.stun_link().await else { return Ok(None) };
    Ok(Some(SwarmLinkView {
        base_url: link.base_url,
        swarms: link.swarms.into_iter().map(|s| SwarmSummaryView { id: s.id, name: s.name }).collect(),
        allowed_peer_count: core.allowed.len(),
    }))
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
    let swarm = core.register_with_stun(&base_url, &code, &device_name).await.map_err(|e| e.to_string())?;
    Ok(SwarmSummaryView { id: swarm.id, name: swarm.name })
}

#[tauri::command]
async fn join_additional_swarm(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    code: String,
) -> Result<SwarmSummaryView, String> {
    let core = state.core(&app).await?;
    let swarm = core.join_additional_swarm(&code).await.map_err(|e| e.to_string())?;
    Ok(SwarmSummaryView { id: swarm.id, name: swarm.name })
}

#[tauri::command]
async fn resync_swarm(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<usize, String> {
    let core = state.core(&app).await?;
    core.resync().await.map_err(|e| e.to_string())
}

/// Leave one joined swarm, keeping the STUN link (and other memberships)
/// intact — see `ServerCore::leave_swarm`.
#[tauri::command]
async fn leave_swarm(app: tauri::AppHandle, state: tauri::State<'_, AppState>, swarm_id: String) -> Result<(), String> {
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
    core.swarm_devices(&swarm_id).await.map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { core: OnceCell::new() })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            choose_media_folder,
            pick_folder_path,
            pick_file_path,
            read_file_bytes,
            list_media_roots,
            add_media_root,
            remove_media_root,
            set_tmdb_api_key,
            get_status,
            rescan,
            list_entries,
            get_artwork_bytes,
            run_scrape,
            rescrape_entry,
            set_manual_metadata,
            upload_artwork,
            get_swarm_link,
            join_swarm,
            join_additional_swarm,
            resync_swarm,
            leave_swarm,
            get_swarm_devices,
        ])
        .run(tauri::generate_context!())
        .expect("failed to launch SWARM Server");
}
