//! Library scanning: allowlist walk → (size, mtime) change detection →
//! sample-fp-v1 fingerprint → tag/probe enrichment → store reconciliation
//! with pending-change tracking. A rename is a delete of the old path plus an
//! add of the new one (entry keys are path-derived by design).

use crate::roots::MediaRoot;
use crate::scrape::artwork;
use crate::store::{ArtworkKind, EntryRecord, Library, MissingDisposition, ScanManifestEntry};
use crate::{classify, probe, tags};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use swarm_core::peer::MediaKind;
use swarm_core::{entry_key, fingerprint};
use tokio::sync::mpsc::Sender;

const MISSING_CONFIRMATION_GRACE_MS: i64 = 60 * 60 * 1_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub unchanged: u64,
}

/// Live progress for [`scan_roots`], in two phases matching its own two
/// passes: `Discovering` while the directory walk across every root is
/// still finding files (no total is knowable yet — real bug, found live:
/// this walk alone can take minutes over a slow network mount (SMB/NFS)
/// with thousands of files, and originally reported nothing at all, so a
/// rescan looked hung the entire time), then `Processing` once that walk
/// finishes and the total is fixed, ticking as each file's fingerprint/
/// probe work completes (`processed` counts every discovered file as it's
/// visited — added/updated/unchanged/skipped all tick the same way — so
/// "X of Y" reflects real linear progress regardless of outcome).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ScanProgressEvent {
    Discovering { found: u64 },
    Processing { processed: u64, total: u64 },
}

/// Optional live progress side-channel for [`scan_roots`], mirroring
/// `scrape::runner::ScrapeProgress` — a bounded `mpsc` sender, not a Tauri
/// type, so this crate stays usable by the headless daemon with zero UI
/// dependency. Intermediate frames may be dropped when the UI falls behind;
/// progress reporting must never grow memory or stall the scan itself.
struct ScanProgress {
    sender: Sender<ScanProgressEvent>,
    found: AtomicU64,
    processed: AtomicU64,
}

impl ScanProgress {
    fn new(sender: Sender<ScanProgressEvent>) -> Self {
        Self {
            sender,
            found: AtomicU64::new(0),
            processed: AtomicU64::new(0),
        }
    }

    /// Called once per file as the directory walk discovers it, before the
    /// total is known.
    fn tick_discovering(&self) {
        let found = self.found.fetch_add(1, Ordering::Relaxed) + 1;
        // Progress is advisory. Never let a slow webview accumulate one
        // queued allocation per file or stall the filesystem walk.
        let _ = self
            .sender
            .try_send(ScanProgressEvent::Discovering { found });
    }

    /// Called once per file as the second pass (fingerprint/probe/store)
    /// visits it, now that `total` (the walk's final file count) is fixed.
    fn tick_processing(&self, total: u64) {
        let processed = self.processed.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self
            .sender
            .try_send(ScanProgressEvent::Processing { processed, total });
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("at least one media root is required")]
    NoMediaRoots,
    #[error("io error under media root: {0}")]
    Io(#[from] std::io::Error),
    #[error("library store error: {0}")]
    Store(#[from] sqlx::Error),
    #[error(
        "found 0 files across every configured root, but the library already has {0} known entries — refusing to \
         treat this as \"everything was deleted\" (a dropped network mount looks identical to an empty root). \
         Check that every media root is actually reachable, then rescan again."
    )]
    SuspiciousEmptyScan(usize),
}

/// Walk a single `root` and reconcile the library with what is on disk.
/// Thin wrapper over [`scan_roots`] for the overwhelmingly common
/// single-root case — see that function and `crate::roots` for why a
/// single root never gets a `{label}/` prefix on `relative_path`.
pub async fn scan_root(library: &Library, root: &Path) -> Result<ScanReport, ScanError> {
    scan_roots(
        library,
        &[MediaRoot {
            label: "local".to_string(),
            path: root.to_path_buf(),
        }],
        None,
    )
    .await
}

/// Walk every configured root and reconcile the library with what is on
/// disk. With 2+ roots, each root's files are stored under a
/// `{label}/`-prefixed `relative_path` (see `crate::roots::RootResolver`) so
/// two roots containing the same sub-path don't collide on `entry_key`; with
/// exactly one root, no prefix is applied and behavior is byte-identical to
/// [`scan_root`]. `progress_tx`, when given, receives best-effort
/// [`ScanProgressEvent`] updates during both phases (directory walk, then
/// per-file fingerprint/probe) — see [`ScanProgress`]'s doc comment.
pub async fn scan_roots(
    library: &Library,
    roots: &[MediaRoot],
    progress_tx: Option<Sender<ScanProgressEvent>>,
) -> Result<ScanReport, ScanError> {
    scan_roots_scoped(library, roots, roots.len() > 1, progress_tx).await
}

/// Reconcile only `roots` while preserving the path namespace of the full
/// configured root set. When `multi_root_namespace` is true, paths retain
/// their `{label}/` prefix and deletion reconciliation is restricted to the
/// supplied labels. This is used after one network root recovers so unrelated
/// local or network roots do not have to be walked again.
pub async fn scan_roots_scoped(
    library: &Library,
    roots: &[MediaRoot],
    multi_root_namespace: bool,
    progress_tx: Option<Sender<ScanProgressEvent>>,
) -> Result<ScanReport, ScanError> {
    if roots.is_empty() {
        return Err(ScanError::NoMediaRoots);
    }
    let scan_id = library.begin_scan_manifest().await?;
    let result =
        scan_roots_scoped_inner(library, roots, multi_root_namespace, progress_tx, &scan_id).await;
    let cleanup = library.clear_scan_manifest(&scan_id).await;
    match (result, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Ok(report), Ok(())) => Ok(report),
    }
}

async fn scan_roots_scoped_inner(
    library: &Library,
    roots: &[MediaRoot],
    multi_root_namespace: bool,
    progress_tx: Option<Sender<ScanProgressEvent>>,
    scan_id: &str,
) -> Result<ScanReport, ScanError> {
    let mut report = ScanReport::default();
    let mut artwork_cache = ArtworkCache::default();
    // Arc, not a bare ScanProgress: discover_media_files below hands a clone
    // across a spawn_blocking boundary, which needs 'static + Send ownership,
    // not a borrow tied to this function's stack frame.
    let progress = progress_tx.map(|tx| Arc::new(ScanProgress::new(tx)));

    // Finish every root walk before catalog mutation, but stage the results
    // in local SQLite rather than growing an in-memory Vec with the library.
    for root in roots {
        discover_media_files(
            library,
            scan_id,
            root,
            multi_root_namespace,
            progress.as_ref(),
        )
        .await?;
    }
    let total = library.scan_manifest_count(scan_id).await?;
    let known_in_scope = if multi_root_namespace {
        let mut count = 0usize;
        for root in roots {
            count = count.saturating_add(
                library
                    .entry_count_with_prefix(Some(&format!("{}/", root.label)))
                    .await?,
            );
        }
        count
    } else {
        library.entry_count_with_prefix(None).await?
    };

    if total == 0 && known_in_scope > 0 {
        return Err(ScanError::SuspiciousEmptyScan(known_in_scope));
    }

    let prefixes = if multi_root_namespace {
        roots
            .iter()
            .map(|root| Some(format!("{}/", root.label)))
            .collect::<Vec<_>>()
    } else {
        vec![None]
    };

    let mut cursor = String::new();
    loop {
        let files = library.scan_manifest_page(scan_id, &cursor, 256).await?;
        if files.is_empty() {
            break;
        }
        for file in files {
            cursor.clone_from(&file.relative_path);
            let relative = file.relative_path.clone();
            let absolute = PathBuf::from(&file.absolute_path);
            if let Some(progress) = &progress {
                progress.tick_processing(total);
            }
            let known = library.known_entry_by_path(&relative).await?;
            if let Some(known_entry) = known.as_ref() {
                if known_entry.size == file.size && known_entry.modified_time == file.modified_time
                {
                    if known_entry.available {
                        report.unchanged += 1;
                    } else if library.restore_available_by_path(&relative).await? {
                        report.added += 1;
                    }
                    if !known_entry.has_artwork {
                        if let Some(classified) = classify::classify(&file.relative_under_root) {
                            recover_existing_artwork(
                                library,
                                &absolute,
                                &entry_key::entry_key(&relative),
                                &relative,
                                classified.kind,
                                &mut artwork_cache,
                            )
                            .await?;
                        }
                    }
                    continue;
                }
            }

            let Some(classified) = classify::classify(&file.relative_under_root) else {
                continue;
            };
            // fingerprint_file/read_tags are synchronous std::fs I/O — each a
            // potential SMB/NFS round trip — and this task shares its tokio
            // worker thread with QUIC/HTTP request handling; spawn_blocking
            // keeps a slow network mount from stalling every other request
            // on the server for as long as this file takes. Only new/changed
            // files reach this line at all (the unchanged fast path above
            // already `continue`d), so this cost is paid exactly where it's
            // unavoidable, not on every file in a routine rescan.
            let fp_path = absolute.clone();
            let Ok(fp) = tokio::task::spawn_blocking(move || fingerprint::fingerprint_file(&fp_path))
                .await
                .expect("fingerprint task panicked")
            else {
                continue;
            };
            let tags_path = absolute.clone();
            let tag = tokio::task::spawn_blocking(move || tags::read_tags(&tags_path))
                .await
                .expect("tag-read task panicked");
            let media = probe::probe(&absolute).await;
            let entry_key = entry_key::entry_key(&relative);
            let mut record = EntryRecord {
                entry_key: entry_key.clone(),
                relative_path: relative,
                kind: classified.kind,
                title: tag
                    .as_ref()
                    .and_then(|tag| tag.title.clone())
                    .unwrap_or(classified.title),
                size: file.size,
                modified_time: file.modified_time,
                fingerprint: fp,
                artist: tag
                    .as_ref()
                    .and_then(|tag| tag.artist.clone())
                    .or(classified.artist),
                album: tag
                    .as_ref()
                    .and_then(|tag| tag.album.clone())
                    .or(classified.album),
                track_number: tag
                    .as_ref()
                    .and_then(|tag| tag.track_number)
                    .or(classified.track_number),
                show_title: classified.show_title,
                season: classified.season,
                episode: classified.episode,
                year: classified.year,
                duration_secs: media.as_ref().and_then(|media| media.duration_secs),
                video: media.as_ref().and_then(|media| media.video.clone()),
                audio: media.as_ref().and_then(|media| media.audio.clone()),
                scraped_title: None,
                episode_title: None,
                genres: Vec::new(),
                artwork_version: 0,
                cast: Vec::new(),
                overview: None,
                rating: None,
                community_rating: None,
                community_rating_votes: None,
            };
            if known.as_ref().is_some_and(|entry| entry.kind_overridden) {
                if let Ok(Some(existing)) = library.get(&entry_key).await {
                    record.kind = existing.kind;
                    record.title = existing.title;
                    record.artist = existing.artist;
                    record.album = existing.album;
                    record.track_number = existing.track_number;
                    record.show_title = existing.show_title;
                    record.season = existing.season;
                    record.episode = existing.episode;
                }
            }
            library.upsert(&record).await?;
            if known.is_none() {
                library
                    .restore_archived_metadata(
                        &record.entry_key,
                        &record.fingerprint,
                        record.size,
                        &record.relative_path,
                    )
                    .await?;
            }
            let already_had_artwork = known.as_ref().is_some_and(|entry| entry.has_artwork);
            if known.as_ref().is_some_and(|entry| entry.available) {
                report.updated += 1;
            } else {
                report.added += 1;
            }
            if !already_had_artwork {
                recover_existing_artwork(
                    library,
                    &absolute,
                    &record.entry_key,
                    &record.relative_path,
                    record.kind,
                    &mut artwork_cache,
                )
                .await?;
            }
        }
    }

    // Only a fully processed manifest may change availability. Missing rows
    // remain as durable tombstones; the active list/catalog hides them while
    // later successful scans advance their confirmation counter.
    for prefix in &prefixes {
        let mut missing_cursor = String::new();
        loop {
            let missing = library
                .paths_missing_from_scan(scan_id, prefix.as_deref(), &missing_cursor, 256)
                .await?;
            if missing.is_empty() {
                break;
            }
            for path in missing {
                missing_cursor.clone_from(&path);
                if matches!(
                    library
                        .mark_missing_by_path(&path, MISSING_CONFIRMATION_GRACE_MS)
                        .await?,
                    Some(MissingDisposition::NewlyMissing)
                ) {
                    report.removed += 1;
                }
            }
        }
    }

    Ok(report)
}

/// Classifies an `images/` sibling file by the exact, small set of
/// filenames every artwork-writing path in this codebase actually produces
/// — `save_video_artwork`'s `{stem}-tmdb-{poster,season-poster,backdrop}.jpg`
/// (`scrape/runner.rs`), `scrape_one_album_group`'s fixed `album-cover.jpg`/
/// `artist-photo.jpg`, and the GUI's manual-upload `manual-{poster,backdrop,
/// cover,artist}.<ext>` (`apps/server/src/gui.rs`, `ArtworkKind::
/// route_segment`). Extension-agnostic (manual uploads aren't always
/// `.jpg`) — matches on the filename stem only.
fn recovered_artwork_kind(filename: &str) -> Option<ArtworkKind> {
    let lower = filename.to_lowercase();
    let stem = lower
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(lower.as_str());
    if stem.ends_with("-tmdb-poster") || stem == "manual-poster" {
        Some(ArtworkKind::Poster)
    } else if stem.ends_with("-tmdb-season-poster") || stem == "manual-season" {
        Some(ArtworkKind::SeasonPoster)
    } else if stem.ends_with("-tmdb-backdrop") || stem == "manual-backdrop" {
        Some(ArtworkKind::Backdrop)
    } else if stem == "album-cover" || stem == "manual-cover" {
        Some(ArtworkKind::Cover)
    } else if stem == "artist-photo" || stem == "manual-artist" {
        Some(ArtworkKind::ArtistPhoto)
    } else {
        None
    }
}

/// Real gap this closes, found live: scraped artwork is saved as a plain
/// file in an `images/` folder right beside the source media (see
/// `scrape::artwork::save_artwork`) — entirely independent of the SQLite
/// catalog. A rescan's `upsert` deliberately never touches artwork columns
/// for an *existing* row (so a normal incremental rescan can't clobber
/// already-scraped data), so any row with every artwork column unset —
/// whether it's a brand-new row (genuinely new file, or the catalog itself
/// was emptied and rebuilt) or an already-known, unchanged row that simply
/// never got relinked (e.g. it was first re-added by a scan that ran
/// *before* this recovery existed) — starts, or stays, looking unscraped
/// regardless of what's physically sitting on disk. Without this, a rebuilt
/// catalog looks completely unscraped even though the real artwork files
/// never moved, and would need a full, redundant re-scrape — real TMDb/
/// MusicBrainz API calls all over again — just to get back images that
/// were already there. Called only when the caller already knows this
/// entry has no artwork set (see both call sites) — an entry that already
/// has some is left alone, correctly untouched.
async fn recover_existing_artwork(
    library: &Library,
    absolute: &Path,
    entry_key: &str,
    relative_path: &str,
    kind: MediaKind,
    artwork_cache: &mut ArtworkCache,
) -> sqlx::Result<()> {
    let Some(parent) = absolute.parent() else {
        return Ok(());
    };
    let images_dir = parent.join("images");
    let candidates = artwork_cache.candidates(&images_dir).await;

    let relevant = |k: ArtworkKind| match kind {
        MediaKind::Movie | MediaKind::Episode => {
            matches!(
                k,
                ArtworkKind::Poster | ArtworkKind::SeasonPoster | ArtworkKind::Backdrop
            )
        }
        MediaKind::Track => matches!(k, ArtworkKind::Cover | ArtworkKind::ArtistPhoto),
    };
    let relative_images_dir = match relative_path.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/images"),
        None => "images".to_string(),
    };
    // Movies/episodes each get their OWN poster/backdrop file, even though
    // several of them can share one `images/` folder (a flat movie root
    // with no per-movie subfolder, or a season folder holding many
    // episodes) — `save_video_artwork` names every file `{own_stem}-tmdb-
    // {poster,backdrop}.jpg`, so that prefix is the only safe way to tell
    // "mine" from "a sibling's". Real bug, found live: without this check,
    // *any* poster sitting in a shared `images/` folder got linked to
    // every sibling recovered from it — one movie's poster ("10 Cloverfield
    // Lane", in the reported case) ended up on many unrelated movies,
    // whichever happened to be recovered after it read the same folder.
    // Tracks are the deliberate exception, not a gap: `album-cover.jpg`/
    // `artist-photo.jpg` really are meant to be shared by every track in
    // the same album folder, so no stem check applies to them. Manual
    // uploads (`manual-poster.<ext>` etc.) have no per-entry stem to check
    // against at all, so they're deliberately excluded from automatic
    // recovery here — a real, separate gap for a manually-uploaded movie
    // poster in a shared folder, not one this fix can safely close too.
    let own_stem_prefix = format!(
        "{}-tmdb-",
        artwork::sanitize_stem(artwork::file_stem(relative_path)).to_lowercase()
    );

    for (name, found_kind) in candidates {
        if !relevant(found_kind) {
            continue;
        }
        if matches!(
            found_kind,
            ArtworkKind::Poster | ArtworkKind::SeasonPoster | ArtworkKind::Backdrop
        ) && !name.to_lowercase().starts_with(&own_stem_prefix)
        {
            continue;
        }
        library
            .set_artwork(
                entry_key,
                found_kind,
                &format!("{relative_images_dir}/{name}"),
            )
            .await?;
    }
    Ok(())
}

/// Path-ordered manifest pages keep sibling media together, so caching just
/// the current artwork directory avoids repeated SMB listings without an
/// unbounded directory-to-files map.
#[derive(Default)]
struct ArtworkCache {
    directory: Option<PathBuf>,
    entries: Vec<(String, ArtworkKind)>,
}

impl ArtworkCache {
    /// Async, not a plain fn, specifically so the cache-miss path's
    /// `std::fs::read_dir` (a real SMB/NFS round trip, same reasoning as
    /// `discover_media_files`'s doc comment) can run via `spawn_blocking`
    /// instead of inline on this shared runtime. The cache-hit path (the
    /// common case — sibling media in the same directory) does no I/O at
    /// all and returns immediately.
    async fn candidates(&mut self, directory: &Path) -> Vec<(String, ArtworkKind)> {
        if self.directory.as_deref() != Some(directory) {
            self.directory = Some(directory.to_path_buf());
            let directory = directory.to_path_buf();
            self.entries = tokio::task::spawn_blocking(move || {
                std::fs::read_dir(&directory)
                    .map(|entries| {
                        entries
                            .flatten()
                            .filter_map(|entry| {
                                let name = entry.file_name().to_string_lossy().into_owned();
                                recovered_artwork_kind(&name).map(|kind| (name, kind))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();
        }
        self.entries.clone()
    }
}

/// Recursive allowlist walk, staged into SQLite in fixed-size batches. Size
/// and modification time are captured while each entry is hot in the SMB
/// client cache, avoiding a second network metadata round trip.
///
/// The walk itself (`walk_media_files`, below) runs entirely inside
/// `spawn_blocking` — never inline on this `async fn`'s own task. Real
/// incident: this task's tokio worker thread is shared with every QUIC/HTTP
/// request this server handles, and `std::fs::read_dir`/`metadata` against a
/// flaky or slow network mount (SMB over a VPN'd link, in the case that
/// found this) can each cost a real round trip with no yield point in
/// between — a single scan blocked every other request on the server for as
/// long as the walk ran, surfacing as artwork/playback requests stalling or
/// timing out client-side, not as a scan-specific symptom. A bounded
/// channel bridges the walk's synchronous batches back to this function's
/// async DB writes; its capacity also bounds how far the walk can run ahead
/// of persistence, the same "bound scan memory" goal the batching itself
/// already serves.
async fn discover_media_files(
    library: &Library,
    scan_id: &str,
    root: &MediaRoot,
    multi_root_namespace: bool,
    progress: Option<&Arc<ScanProgress>>,
) -> Result<(), ScanError> {
    const BATCH_SIZE: usize = 256;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<ScanManifestEntry>>(2);
    let root_path = root.path.clone();
    let label = root.label.clone();
    let progress = progress.cloned();
    let walk = tokio::task::spawn_blocking(move || {
        walk_media_files(
            &root_path,
            &label,
            multi_root_namespace,
            progress.as_deref(),
            BATCH_SIZE,
            tx,
        )
    });

    while let Some(batch) = rx.recv().await {
        library.append_scan_manifest(scan_id, &batch).await?;
    }
    walk.await.expect("media walk task panicked")?;
    Ok(())
}

/// Pure synchronous directory walk — see [`discover_media_files`]'s doc
/// comment for why this must only ever run via `spawn_blocking`, never
/// inline on the shared async runtime. Sends completed batches through `tx`
/// as it goes; `Sender::blocking_send` blocks *this* (already-blocking-pool)
/// thread, not any async worker thread, when the channel is full, which is
/// exactly the backpressure a bounded channel is for.
fn walk_media_files(
    root_path: &Path,
    label: &str,
    multi_root_namespace: bool,
    progress: Option<&ScanProgress>,
    batch_size: usize,
    tx: Sender<Vec<ScanManifestEntry>>,
) -> std::io::Result<()> {
    let mut batch = Vec::with_capacity(batch_size);
    let mut stack = vec![root_path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(relative) = path.strip_prefix(root_path) else {
                continue;
            };
            let relative_under_root = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if classify::media_extension(&relative_under_root).is_some() {
                let metadata = entry.metadata()?;
                let modified_time = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs() as i64)
                    .unwrap_or(0);
                if let Some(p) = progress {
                    p.tick_discovering();
                }
                let relative_path = if multi_root_namespace {
                    format!("{label}/{relative_under_root}")
                } else {
                    relative_under_root.clone()
                };
                batch.push(ScanManifestEntry {
                    relative_path,
                    absolute_path: path.to_string_lossy().into_owned(),
                    relative_under_root,
                    size: metadata.len(),
                    modified_time,
                });
                if batch.len() == batch_size {
                    let full_batch = std::mem::replace(&mut batch, Vec::with_capacity(batch_size));
                    // Receiver gone means discover_media_files already
                    // failed on an earlier DB write — nothing left to hand
                    // batches to, so stop walking rather than keep working
                    // toward a result no one will read.
                    if tx.blocking_send(full_batch).is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
    if !batch.is_empty() {
        let _ = tx.blocking_send(batch);
    }
    Ok(())
}
