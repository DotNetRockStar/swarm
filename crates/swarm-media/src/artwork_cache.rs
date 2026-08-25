//! Live observability for the optional server-local artwork cache.
//!
//! Cache fills and hits are retained for one hour so the desktop dashboard
//! can render the same kind of rolling view as streaming bandwidth. Disk
//! usage is measured on demand instead of maintained as a second source of
//! truth, which also accounts for generated thumbnails and files left by an
//! interrupted prior process.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HISTORY_AGE: Duration = Duration::from_secs(60 * 60);
const MAX_EVENTS: usize = 50_000;
const DISK_USAGE_REFRESH: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkCacheEventKind {
    Cached,
    ServedFromCache,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtworkCacheEvent {
    pub timestamp_ms: i64,
    pub client: String,
    pub kind: ArtworkCacheEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtworkCacheSnapshot {
    pub enabled: bool,
    pub cache_dir: String,
    pub disk_bytes: u64,
    pub file_count: u64,
    pub events: Vec<ArtworkCacheEvent>,
}

pub struct ArtworkCacheMonitor {
    cache_dir: Option<PathBuf>,
    events: Mutex<VecDeque<ArtworkCacheEvent>>,
    disk_usage: tokio::sync::Mutex<Option<DiskUsageSample>>,
}

struct DiskUsageSample {
    measured_at: Instant,
    bytes: u64,
    files: u64,
}

impl ArtworkCacheMonitor {
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self {
            cache_dir,
            events: Mutex::new(VecDeque::new()),
            disk_usage: tokio::sync::Mutex::new(None),
        }
    }

    pub fn record(&self, client: &str, kind: ArtworkCacheEventKind) {
        let timestamp_ms = unix_time_ms();
        let mut events = self.events.lock().unwrap();
        prune(&mut events, timestamp_ms);
        if events.len() == MAX_EVENTS {
            events.pop_front();
        }
        events.push_back(ArtworkCacheEvent {
            timestamp_ms,
            client: display_client(client),
            kind,
        });
    }

    pub async fn snapshot(&self, enabled: bool) -> ArtworkCacheSnapshot {
        let cache_dir = self.cache_dir.clone();
        let display_dir = cache_dir
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (disk_bytes, file_count) = self.disk_usage(cache_dir).await;
        let now = unix_time_ms();
        let events = {
            let mut events = self.events.lock().unwrap();
            prune(&mut events, now);
            events.iter().cloned().collect()
        };
        ArtworkCacheSnapshot {
            enabled,
            cache_dir: display_dir,
            disk_bytes,
            file_count,
            events,
        }
    }

    async fn disk_usage(&self, cache_dir: Option<PathBuf>) -> (u64, u64) {
        let mut cached = self.disk_usage.lock().await;
        if let Some(sample) = cached.as_ref() {
            if sample.measured_at.elapsed() < DISK_USAGE_REFRESH {
                return (sample.bytes, sample.files);
            }
        }
        let usage = match cache_dir {
            Some(path) => tokio::task::spawn_blocking(move || disk_usage(&path))
                .await
                .unwrap_or((0, 0)),
            None => (0, 0),
        };
        *cached = Some(DiskUsageSample {
            measured_at: Instant::now(),
            bytes: usage.0,
            files: usage.1,
        });
        usage
    }
}

fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn display_client(client: &str) -> String {
    let client = client.trim();
    if client.is_empty() {
        "Unknown client".to_string()
    } else {
        client.chars().take(120).collect()
    }
}

fn prune(events: &mut VecDeque<ArtworkCacheEvent>, now_ms: i64) {
    let cutoff = now_ms.saturating_sub(HISTORY_AGE.as_millis() as i64);
    while events
        .front()
        .is_some_and(|event| event.timestamp_ms < cutoff)
    {
        events.pop_front();
    }
}

fn disk_usage(root: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return (0, 0);
    };
    let mut bytes = 0u64;
    let mut files = 0u64;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let (child_bytes, child_files) = disk_usage(&entry.path());
            bytes = bytes.saturating_add(child_bytes);
            files = files.saturating_add(child_files);
        } else if file_type.is_file() {
            if let Ok(metadata) = entry.metadata() {
                bytes = bytes.saturating_add(metadata.len());
                files = files.saturating_add(1);
            }
        }
    }
    (bytes, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn snapshot_reports_events_and_recursive_disk_usage() {
        let root = std::env::temp_dir().join(format!(
            "swarm-artwork-cache-monitor-{}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(root.join("ab/.swarm-thumbnails")).unwrap();
        std::fs::write(root.join("ab/poster.jpg"), [1u8, 2, 3]).unwrap();
        std::fs::write(root.join("ab/.swarm-thumbnails/poster-w320.jpg"), [4u8, 5]).unwrap();

        let monitor = ArtworkCacheMonitor::new(Some(root.clone()));
        monitor.record(" Living Room TV ", ArtworkCacheEventKind::Cached);
        monitor.record("Living Room TV", ArtworkCacheEventKind::ServedFromCache);
        let snapshot = monitor.snapshot(true).await;

        assert!(snapshot.enabled);
        assert_eq!(snapshot.disk_bytes, 5);
        assert_eq!(snapshot.file_count, 2);
        assert_eq!(snapshot.events.len(), 2);
        assert_eq!(snapshot.events[0].client, "Living Room TV");
        assert_eq!(snapshot.events[0].kind, ArtworkCacheEventKind::Cached);
        assert_eq!(
            snapshot.events[1].kind,
            ArtworkCacheEventKind::ServedFromCache
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
