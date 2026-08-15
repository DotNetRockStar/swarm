//! Persisted GUI settings: media root and TMDb API key. Kept separate from
//! `ServerCore`'s own state (STUN link, library) because these are needed
//! *before* a core can even be constructed — the packaged app has no
//! `SWARM_MEDIA_ROOT` env var to fall back on, unlike the headless daemon.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub media_root: Option<String>,
    pub tmdb_api_key: Option<String>,
}

fn settings_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("settings.json")
}

pub fn load(app_data_dir: &Path) -> Settings {
    std::fs::read_to_string(settings_path(app_data_dir))
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

pub fn save(app_data_dir: &Path, settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(app_data_dir)?;
    let json = serde_json::to_string_pretty(settings).unwrap_or_default();
    std::fs::write(settings_path(app_data_dir), json)
}
