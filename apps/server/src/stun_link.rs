//! Persisted link to a SWARM STUN server: which server, which swarms. The
//! access token itself lives in `TokenStore`, not here — this file is not a
//! secret and is fine to read while debugging.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use swarm_core::rest::SwarmSummary;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StunLinkRecord {
    pub base_url: String,
    pub device_id: String,
    pub swarms: Vec<SwarmSummary>,
}

fn link_path(data_dir: &Path) -> PathBuf {
    data_dir.join("stun-link.json")
}

pub fn load(data_dir: &Path) -> Option<StunLinkRecord> {
    let contents = std::fs::read_to_string(link_path(data_dir)).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn save(data_dir: &Path, record: &StunLinkRecord) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(record).unwrap_or_default();
    std::fs::write(link_path(data_dir), json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("swarm-link-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load(&dir).is_none());
        let record = StunLinkRecord {
            base_url: "https://swarm.example.com".into(),
            device_id: "dev-1".into(),
            swarms: vec![SwarmSummary { id: "sw-1".into(), name: "Home".into() }],
        };
        save(&dir, &record).unwrap();
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.device_id, "dev-1");
        assert_eq!(loaded.swarms.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
