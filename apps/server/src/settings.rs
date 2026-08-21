//! Persisted GUI settings: media root(s) and TMDb API key. Kept separate
//! from `ServerCore`'s own state (STUN link, library) because these are
//! needed *before* a core can even be constructed — the packaged app has no
//! `SWARM_MEDIA_ROOT` env var to fall back on, unlike the headless daemon.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRootSetting {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub media_roots: Vec<MediaRootSetting>,
    /// Superseded by `media_roots` — read-only now, kept only so an older
    /// settings.json (single-root, pre-this-field) still loads. Never
    /// written by this build; see `load`'s one-time upgrade below.
    #[serde(default)]
    media_root: Option<String>,
    pub tmdb_api_key: Option<String>,
    /// Whether the read-only MCP server (see `mcp.rs`) starts alongside the
    /// GUI app's core. Takes effect on next launch/restart — no hot-reload,
    /// same posture as `media_root`'s pre-multi-root upgrade above having no
    /// live-apply path either.
    #[serde(default)]
    pub mcp_enabled: bool,
    #[serde(default = "default_mcp_port")]
    pub mcp_port: u16,
}

fn default_mcp_port() -> u16 {
    7890
}

// Hand-written rather than `#[derive(Default)]` so a brand-new install (no
// settings.json at all, going through `Settings::default()` in `load` below)
// gets the exact same `mcp_port` default as an *existing* settings.json
// that simply predates this field (going through serde's per-field
// `#[serde(default = "default_mcp_port")]`) — a derived `Default` would
// silently disagree (`u16::default()` is `0`, not `default_mcp_port()`).
impl Default for Settings {
    fn default() -> Self {
        Settings { media_roots: Vec::new(), media_root: None, tmdb_api_key: None, mcp_enabled: false, mcp_port: default_mcp_port() }
    }
}

fn settings_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("settings.json")
}

/// Loads persisted settings, transparently upgrading a pre-multi-root
/// settings.json (single `media_root: Option<String>`, no `media_roots`)
/// into the new shape in memory. Not written back to disk here — the next
/// `save` call (any settings change) persists the upgraded shape naturally.
pub fn load(app_data_dir: &Path) -> Settings {
    let mut settings: Settings = std::fs::read_to_string(settings_path(app_data_dir))
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default();
    if settings.media_roots.is_empty() {
        if let Some(path) = settings.media_root.take() {
            settings.media_roots.push(MediaRootSetting { label: "local".to_string(), path });
        }
    }
    settings
}

pub fn save(app_data_dir: &Path, settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(app_data_dir)?;
    let json = serde_json::to_string_pretty(settings).unwrap_or_default();
    std::fs::write(settings_path(app_data_dir), json)
}
