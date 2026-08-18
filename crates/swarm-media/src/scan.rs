//! Library scanning: allowlist walk → (size, mtime) change detection →
//! sample-fp-v1 fingerprint → tag/probe enrichment → store reconciliation
//! with pending-change tracking. A rename is a delete of the old path plus an
//! add of the new one (entry keys are path-derived by design).

use crate::roots::MediaRoot;
use crate::store::{EntryRecord, Library};
use crate::{classify, probe, tags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use swarm_core::{entry_key, fingerprint};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub unchanged: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("io error under media root: {0}")]
    Io(#[from] std::io::Error),
    #[error("library store error: {0}")]
    Store(#[from] sqlx::Error),
}

/// Walk a single `root` and reconcile the library with what is on disk.
/// Thin wrapper over [`scan_roots`] for the overwhelmingly common
/// single-root case — see that function and `crate::roots` for why a
/// single root never gets a `{label}/` prefix on `relative_path`.
pub async fn scan_root(library: &Library, root: &Path) -> Result<ScanReport, ScanError> {
    scan_roots(library, &[MediaRoot { label: "local".to_string(), path: root.to_path_buf() }]).await
}

/// Walk every configured root and reconcile the library with what is on
/// disk. With 2+ roots, each root's files are stored under a
/// `{label}/`-prefixed `relative_path` (see `crate::roots::RootResolver`) so
/// two roots containing the same sub-path don't collide on `entry_key`; with
/// exactly one root, no prefix is applied and behavior is byte-identical to
/// [`scan_root`].
pub async fn scan_roots(library: &Library, roots: &[MediaRoot]) -> Result<ScanReport, ScanError> {
    let mut report = ScanReport::default();
    let known = library.snapshot().await?;
    let mut seen: HashMap<String, ()> = HashMap::new();
    let multi = roots.len() > 1;

    for root in roots {
        let files = collect_media_files(&root.path)?;
        for (absolute, relative_under_root) in files {
            let relative =
                if multi { format!("{}/{relative_under_root}", root.label) } else { relative_under_root };
            let metadata = match std::fs::metadata(&absolute) {
                Ok(meta) => meta,
                Err(_) => continue, // vanished mid-scan
            };
            let size = metadata.len();
            let mtime = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            seen.insert(relative.clone(), ());

            if let Some((known_size, known_mtime, _)) = known.get(&relative) {
                if *known_size == size && *known_mtime == mtime {
                    report.unchanged += 1;
                    continue;
                }
            }

            let Some(classified) = classify::classify(&relative) else { continue };
            let Ok(fp) = fingerprint::fingerprint_file(&absolute) else { continue };
            // Embedded tags override the path-derived *display* fields when
            // present; grouping keys stay path-derived upstream of this.
            let tag = tags::read_tags(&absolute);
            let media = probe::probe(&absolute).await;
            let record = EntryRecord {
                entry_key: entry_key::entry_key(&relative),
                relative_path: relative,
                kind: classified.kind,
                title: tag.as_ref().and_then(|t| t.title.clone()).unwrap_or(classified.title),
                size,
                modified_time: mtime,
                fingerprint: fp,
                artist: tag.as_ref().and_then(|t| t.artist.clone()).or(classified.artist),
                album: tag.as_ref().and_then(|t| t.album.clone()).or(classified.album),
                track_number: tag.as_ref().and_then(|t| t.track_number).or(classified.track_number),
                show_title: classified.show_title,
                season: classified.season,
                episode: classified.episode,
                year: classified.year,
                duration_secs: media.as_ref().and_then(|m| m.duration_secs),
                video: media.as_ref().and_then(|m| m.video.clone()),
                audio: media.as_ref().and_then(|m| m.audio.clone()),
                // Scraper/artwork fields are not written by `upsert` (a rescan
                // must never clobber existing scrape results), so these values
                // are unused placeholders on the write path.
                scraped_title: None,
                genres: Vec::new(),
                artwork_version: 0,
                cast: Vec::new(),
            };
            library.upsert(&record).await?;
            if known.contains_key(&record.relative_path) {
                report.updated += 1;
            } else {
                report.added += 1;
            }
        }
    }

    for path in known.keys() {
        if !seen.contains_key(path) {
            library.remove_by_path(path).await?;
            report.removed += 1;
        }
    }
    Ok(report)
}

/// Recursive allowlist walk. Skips symlinks, hidden entries, and anything
/// without a known media extension. Paths come back as (absolute,
/// forward-slash relative).
fn collect_media_files(root: &Path) -> std::io::Result<Vec<(PathBuf, String)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue, // unreadable subdir: skip, don't fail the scan
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else { continue };
            let relative = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if classify::media_extension(&relative).is_some() {
                out.push((path, relative));
            }
        }
    }
    Ok(out)
}
