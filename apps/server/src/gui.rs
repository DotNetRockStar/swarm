//! Tauri desktop shell for the SWARM media server (feature `gui`).
//!
//! Thin windowing over [`swarm_server::ServerCore`]: the webview UI (in
//! `ui/`) calls the commands below via `window.__TAURI__.core.invoke`. Until
//! onboarding UI lands, configuration still comes from the same env vars as
//! the daemon; launched without `SWARM_MEDIA_ROOT` the window explains what
//! to set.

use std::sync::Arc;
use swarm_server::{ServerCore, ServerStatus};
use tokio::sync::OnceCell;

struct AppState {
    core: OnceCell<Arc<ServerCore>>,
}

impl AppState {
    async fn core(&self) -> Result<Arc<ServerCore>, String> {
        self.core
            .get_or_try_init(|| async {
                let config = swarm_server::config_from_env()
                    .ok_or_else(|| "SWARM_MEDIA_ROOT is not set — launch with SWARM_MEDIA_ROOT=/path/to/media".to_string())?;
                let (core, _report) = ServerCore::start(config).await.map_err(|e| e.to_string())?;
                Ok(Arc::new(core))
            })
            .await
            .cloned()
    }
}

#[tauri::command]
async fn get_status(state: tauri::State<'_, AppState>) -> Result<ServerStatus, String> {
    state.core().await?.status().await.map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct RescanResult {
    added: u64,
    updated: u64,
    removed: u64,
    unchanged: u64,
}

#[tauri::command]
async fn rescan(state: tauri::State<'_, AppState>) -> Result<RescanResult, String> {
    let report = state.core().await?.rescan().await.map_err(|e| e.to_string())?;
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
}

#[tauri::command]
async fn list_entries(state: tauri::State<'_, AppState>) -> Result<Vec<EntrySummary>, String> {
    let core = state.core().await?;
    let entries = core.library.list().await.map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .map(|entry| EntrySummary {
            entry_key: entry.entry_key,
            kind: format!("{:?}", entry.kind).to_lowercase(),
            title: entry.title,
            relative_path: entry.relative_path,
            size: entry.size,
        })
        .collect())
}

fn main() {
    tauri::Builder::default()
        .manage(AppState { core: OnceCell::new() })
        .invoke_handler(tauri::generate_handler![get_status, rescan, list_entries])
        .run(tauri::generate_context!())
        .expect("failed to launch SWARM Server");
}
