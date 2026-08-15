//! Stable machine identity submitted at registration, so re-registering the
//! same install updates one device row instead of creating duplicates.
//!
//! Batocera.Drone derives this from the first physical NIC's MAC address so
//! identity survives a full reinstall with no local state. SWARM devices
//! already persist state in a data directory (the device certificate lives
//! there too — see `swarm_p2p::identity`), so a random id generated once and
//! persisted alongside it is simpler and equally stable across restarts,
//! at the cost of not surviving a wipe of that directory. Good enough for
//! v1; a MAC-based fallback can be added later without changing the wire
//! format (`machine_id` is an opaque string).

use rand::rngs::OsRng;
use rand::RngCore;
use std::path::Path;

pub fn ensure_machine_id(dir: &Path) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("machine-id");
    match std::fs::read_to_string(&path) {
        Ok(existing) if !existing.trim().is_empty() => Ok(existing.trim().to_string()),
        _ => {
            let mut bytes = [0u8; 16];
            OsRng.fill_bytes(&mut bytes);
            let id = hex::encode(bytes);
            std::fs::write(&path, &id)?;
            Ok(id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_across_calls_and_unique_per_dir() {
        let dir_a = std::env::temp_dir().join(format!("swarm-mid-a-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("swarm-mid-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);

        let first = ensure_machine_id(&dir_a).unwrap();
        let second = ensure_machine_id(&dir_a).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 32);

        let other = ensure_machine_id(&dir_b).unwrap();
        assert_ne!(first, other);

        std::fs::remove_dir_all(&dir_a).ok();
        std::fs::remove_dir_all(&dir_b).ok();
    }
}
