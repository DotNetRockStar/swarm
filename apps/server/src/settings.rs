//! Persisted GUI settings: media root(s) and TMDb API key. Kept separate
//! from `ServerCore`'s own state (STUN link, library) because these are
//! needed *before* a core can even be constructed. The packaged desktop app
//! deliberately has no environment-only media-root configuration path.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRootSetting {
    pub label: String,
    pub path: String,
}

/// A lightweight accessibility check used by the desktop UI. Opening the
/// directory (rather than merely checking `Path::exists`) also catches a
/// mounted network share that is present but no longer readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaRootHealth {
    pub label: String,
    pub path: String,
    pub available: bool,
    pub error: Option<String>,
}

pub fn media_root_health(roots: &[MediaRootSetting]) -> Vec<MediaRootHealth> {
    roots
        .iter()
        .map(|root| match std::fs::read_dir(&root.path) {
            Ok(_) => MediaRootHealth {
                label: root.label.clone(),
                path: root.path.clone(),
                available: true,
                error: None,
            },
            Err(error) => MediaRootHealth {
                label: root.label.clone(),
                path: root.path.clone(),
                available: false,
                error: Some(error.to_string()),
            },
        })
        .collect()
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
    /// Applies the measured upload budget to internet playback. LAN
    /// playback always bypasses it regardless of this setting.
    #[serde(default = "default_streaming_upload_budget_enabled")]
    pub streaming_upload_budget_enabled: bool,
    /// Generate English subtitles locally with Whisper. The model and queue
    /// live in app data; disabling pauses work without discarding progress.
    #[serde(default)]
    pub local_transcription_enabled: bool,
    /// Protect playback from Whisper's sustained CPU load. Users with CPU
    /// headroom can opt out and allow both workloads to run concurrently.
    #[serde(default = "default_transcription_pause_while_streaming")]
    pub transcription_pause_while_streaming: bool,
    /// Whether the read-only MCP server (see `mcp.rs`) starts alongside the
    /// GUI app's core. Takes effect on next launch/restart — no hot-reload,
    /// same posture as `media_root`'s pre-multi-root upgrade above having no
    /// live-apply path either.
    #[serde(default)]
    pub mcp_enabled: bool,
    #[serde(default = "default_mcp_port")]
    pub mcp_port: u16,
    /// Bearer token required by every MCP request.
    #[serde(default)]
    pub mcp_access_token: Option<String>,
}

fn default_streaming_upload_budget_enabled() -> bool {
    true
}

fn default_transcription_pause_while_streaming() -> bool {
    true
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
        Settings {
            media_roots: Vec::new(),
            media_root: None,
            tmdb_api_key: None,
            streaming_upload_budget_enabled: true,
            local_transcription_enabled: false,
            transcription_pause_while_streaming: true,
            mcp_enabled: false,
            mcp_port: default_mcp_port(),
            mcp_access_token: None,
        }
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
            settings.media_roots.push(MediaRootSetting {
                label: "local".to_string(),
                path,
            });
        }
    }
    settings
}

pub fn save(app_data_dir: &Path, settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(app_data_dir)?;
    let json = serde_json::to_string_pretty(settings).unwrap_or_default();
    let path = settings_path(app_data_dir);
    std::fs::write(&path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_settings_keep_the_upload_budget_enabled() {
        let dir =
            std::env::temp_dir().join(format!("swarm-settings-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            settings_path(&dir),
            r#"{"media_roots":[{"label":"local","path":"/media"}],"tmdb_api_key":null,"mcp_enabled":false,"mcp_port":7890}"#,
        )
        .unwrap();
        let loaded = load(&dir);
        assert!(loaded.streaming_upload_budget_enabled);
        assert!(!loaded.local_transcription_enabled);
        assert!(loaded.transcription_pause_while_streaming);
        assert_eq!(loaded.mcp_access_token, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn media_root_health_reports_readable_and_missing_roots() {
        let dir =
            std::env::temp_dir().join(format!("swarm-root-health-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("disconnected-share");
        let roots = vec![
            MediaRootSetting {
                label: "local".into(),
                path: dir.to_string_lossy().into_owned(),
            },
            MediaRootSetting {
                label: "nas".into(),
                path: missing.to_string_lossy().into_owned(),
            },
        ];

        let health = media_root_health(&roots);
        assert!(health[0].available);
        assert_eq!(health[0].error, None);
        assert!(!health[1].available);
        assert!(health[1].error.is_some());

        std::fs::remove_dir_all(dir).unwrap();
    }
}
