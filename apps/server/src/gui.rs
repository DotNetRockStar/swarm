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

use settings::Settings;
use std::path::PathBuf;
use std::sync::Arc;
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
                let media_root = settings::load(&dir).media_root.ok_or_else(|| "not_configured".to_string())?;
                let config = ServerConfig {
                    media_root: PathBuf::from(media_root),
                    data_dir: dir,
                    bind: "0.0.0.0:8543".parse().unwrap(),
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
    media_root: Option<String>,
    has_tmdb_key: bool,
}

#[tauri::command]
async fn get_settings(app: tauri::AppHandle) -> Result<SettingsView, String> {
    let settings = settings::load(&app_data_dir(&app)?);
    Ok(SettingsView { media_root: settings.media_root, has_tmdb_key: settings.tmdb_api_key.is_some() })
}

/// Opens the native folder picker and persists the choice. Does not affect
/// an already-running core — see the module docs.
#[tauri::command]
async fn choose_media_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    let Some(picked) = rx.await.map_err(|e| e.to_string())? else { return Ok(None) };
    let path = picked.to_string();
    let dir = app_data_dir(&app)?;
    let mut settings = settings::load(&dir);
    settings.media_root = Some(path.clone());
    settings::save(&dir, &settings).map_err(|e| e.to_string())?;
    Ok(Some(path))
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
        })
        .collect())
}

#[tauri::command]
async fn run_scrape(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<BulkScrapeReport, String> {
    let core = state.core(&app).await?;
    let tmdb_api_key = settings::load(&app_data_dir(&app)?).tmdb_api_key;
    core.run_scrape(ScrapeConfig { tmdb_api_key, ..Default::default() }).await.map_err(|e| e.to_string())
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { core: OnceCell::new() })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            choose_media_folder,
            set_tmdb_api_key,
            get_status,
            rescan,
            list_entries,
            run_scrape,
            get_swarm_link,
            join_swarm,
            join_additional_swarm,
            resync_swarm,
        ])
        .run(tauri::generate_context!())
        .expect("failed to launch SWARM Server");
}
