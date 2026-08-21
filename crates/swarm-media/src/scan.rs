//! Library scanning: allowlist walk → (size, mtime) change detection →
//! sample-fp-v1 fingerprint → tag/probe enrichment → store reconciliation
//! with pending-change tracking. A rename is a delete of the old path plus an
//! add of the new one (entry keys are path-derived by design).

use crate::roots::MediaRoot;
use crate::scrape::artwork;
use crate::store::{ArtworkKind, EntryRecord, Library};
use crate::{classify, probe, tags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use swarm_core::peer::MediaKind;
use swarm_core::{entry_key, fingerprint};
use tokio::sync::mpsc::UnboundedSender;

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
/// `scrape::runner::ScrapeProgress` — a plain `mpsc` sender, not a Tauri
/// type, so this crate stays usable by the headless daemon with zero UI
/// dependency; the GUI layer turns received events into `app.emit(...)`
/// calls.
struct ScanProgress {
    sender: UnboundedSender<ScanProgressEvent>,
    found: AtomicU64,
    processed: AtomicU64,
}

impl ScanProgress {
    fn new(sender: UnboundedSender<ScanProgressEvent>) -> Self {
        Self { sender, found: AtomicU64::new(0), processed: AtomicU64::new(0) }
    }

    /// Called once per file as the directory walk discovers it, before the
    /// total is known.
    fn tick_discovering(&self) {
        let found = self.found.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.sender.send(ScanProgressEvent::Discovering { found });
    }

    /// Called once per file as the second pass (fingerprint/probe/store)
    /// visits it, now that `total` (the walk's final file count) is fixed.
    fn tick_processing(&self, total: u64) {
        let processed = self.processed.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.sender.send(ScanProgressEvent::Processing { processed, total });
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
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
    scan_roots(library, &[MediaRoot { label: "local".to_string(), path: root.to_path_buf() }], None).await
}

/// Walk every configured root and reconcile the library with what is on
/// disk. With 2+ roots, each root's files are stored under a
/// `{label}/`-prefixed `relative_path` (see `crate::roots::RootResolver`) so
/// two roots containing the same sub-path don't collide on `entry_key`; with
/// exactly one root, no prefix is applied and behavior is byte-identical to
/// [`scan_root`]. `progress_tx`, when given, receives one [`ScanProgressEvent`]
/// per file at each of the two phases (directory walk, then per-file
/// fingerprint/probe) — see [`ScanProgress`]'s doc comment.
pub async fn scan_roots(
    library: &Library,
    roots: &[MediaRoot],
    progress_tx: Option<UnboundedSender<ScanProgressEvent>>,
) -> Result<ScanReport, ScanError> {
    let mut report = ScanReport::default();
    let known = library.snapshot().await?;
    let mut seen: HashMap<String, ()> = HashMap::new();
    let multi = roots.len() > 1;

    let progress = progress_tx.map(ScanProgress::new);

    // Every root's directory walk happens up front so the total file count
    // is known before the per-file processing phase's progress starts —
    // see [`ScanProgress`]'s doc comment for why the walk itself is now
    // also instrumented (`Discovering`), not just this second pass.
    let mut all_files: Vec<(&MediaRoot, PathBuf, String)> = Vec::new();
    for root in roots {
        for (absolute, relative_under_root) in collect_media_files(&root.path, progress.as_ref())? {
            all_files.push((root, absolute, relative_under_root));
        }
    }
    let total = all_files.len() as u64;

    // Belt-and-suspenders alongside `collect_media_files`'s own root-read
    // error: even a *successful* but suspiciously-empty listing (a network
    // share that's connected but returns a transient empty directory, or
    // any other root-level glitch that isn't a hard filesystem error) must
    // never be allowed to reach the removal-reconciliation loop below,
    // which would otherwise read "found nothing" as "the user deleted
    // every file" and wipe the whole known library.
    if total == 0 && !known.is_empty() {
        return Err(ScanError::SuspiciousEmptyScan(known.len()));
    }

    for (root, absolute, relative_under_root) in all_files {
        let relative = if multi { format!("{}/{relative_under_root}", root.label) } else { relative_under_root.clone() };
        if let Some(p) = &progress {
            p.tick_processing(total);
        }
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

        if let Some(known_entry) = known.get(&relative) {
            if known_entry.size == size && known_entry.modified_time == mtime {
                report.unchanged += 1;
                if !known_entry.has_artwork {
                    // Cheap, pure, no I/O — just enough to know `kind` for
                    // the recovery check below, without paying for a real
                    // fingerprint/probe pass on a file that hasn't changed.
                    if let Some(classified) = classify::classify(&relative_under_root) {
                        let entry_key = entry_key::entry_key(&relative);
                        recover_existing_artwork(library, &absolute, &entry_key, &relative, classified.kind).await?;
                    }
                }
                continue;
            }
        }

        // Classified purely from the path *under this root* — never the
        // `{label}/`-prefixed stored form. classify()'s audio branch
        // anchors artist/album from the top-most folder; in the
        // multi-root case that top folder would otherwise be this
        // root's own arbitrary label (e.g. "nas-music"), not a real
        // artist name.
        let Some(classified) = classify::classify(&relative_under_root) else { continue };
        let Ok(fp) = fingerprint::fingerprint_file(&absolute) else { continue };
        // Embedded tags override the path-derived *display* fields when
        // present; grouping keys stay path-derived upstream of this.
        let tag = tags::read_tags(&absolute);
        let media = probe::probe(&absolute).await;
        let entry_key = entry_key::entry_key(&relative);
        let mut record = EntryRecord {
            entry_key: entry_key.clone(),
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
            overview: None,
            rating: None,
        };
        // A manually reclassified file (see `Library::set_manual_kind`) that
        // changed on disk (re-encoded, replaced) would otherwise have its
        // kind/grouping silently reverted here by the fresh path-derived
        // `classified` above — the *file content* changing is real and
        // still needs a fresh fingerprint/probe pass, but the human's
        // classification of *what it is* takes precedence over the path
        // heuristic that got it wrong in the first place.
        if known.get(&record.relative_path).is_some_and(|k| k.kind_overridden) {
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
        let already_had_artwork = known.get(&record.relative_path).is_some_and(|k| k.has_artwork);
        if known.contains_key(&record.relative_path) {
            report.updated += 1;
        } else {
            report.added += 1;
        }
        // A brand-new row never has artwork columns set, since `upsert`
        // deliberately never writes them. An existing row whose content
        // actually changed (`upsert` took the `updated` path) might
        // already have artwork from a prior scrape — only bother checking
        // disk when it doesn't, so an already-linked entry is never
        // touched. See the recovery function's own doc comment for why
        // either case (added or updated-but-still-unscraped) is worth it.
        if !already_had_artwork {
            recover_existing_artwork(library, &absolute, &record.entry_key, &record.relative_path, record.kind).await?;
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

/// Classifies an `images/` sibling file by the exact, small set of
/// filenames every artwork-writing path in this codebase actually produces
/// — `save_video_artwork`'s `{stem}-tmdb-{poster,backdrop}.jpg`
/// (`scrape/runner.rs`), `scrape_one_album_group`'s fixed `album-cover.jpg`/
/// `artist-photo.jpg`, and the GUI's manual-upload `manual-{poster,backdrop,
/// cover,artist}.<ext>` (`apps/server/src/gui.rs`, `ArtworkKind::
/// route_segment`). Extension-agnostic (manual uploads aren't always
/// `.jpg`) — matches on the filename stem only.
fn recovered_artwork_kind(filename: &str) -> Option<ArtworkKind> {
    let lower = filename.to_lowercase();
    let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(lower.as_str());
    if stem.ends_with("-tmdb-poster") || stem == "manual-poster" {
        Some(ArtworkKind::Poster)
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
) -> sqlx::Result<()> {
    let Some(parent) = absolute.parent() else { return Ok(()) };
    let Ok(entries) = std::fs::read_dir(parent.join("images")) else { return Ok(()) };

    let relevant = |k: ArtworkKind| match kind {
        MediaKind::Movie | MediaKind::Episode => matches!(k, ArtworkKind::Poster | ArtworkKind::Backdrop),
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
    let own_stem_prefix = format!("{}-tmdb-", artwork::sanitize_stem(artwork::file_stem(relative_path)).to_lowercase());

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(found_kind) = recovered_artwork_kind(&name) else { continue };
        if !relevant(found_kind) {
            continue;
        }
        if matches!(found_kind, ArtworkKind::Poster | ArtworkKind::Backdrop)
            && !name.to_lowercase().starts_with(&own_stem_prefix)
        {
            continue;
        }
        library.set_artwork(entry_key, found_kind, &format!("{relative_images_dir}/{name}")).await?;
    }
    Ok(())
}

/// Recursive allowlist walk. Skips symlinks, hidden entries, and anything
/// without a known media extension. Paths come back as (absolute,
/// forward-slash relative).
fn collect_media_files(root: &Path, progress: Option<&ScanProgress>) -> std::io::Result<Vec<(PathBuf, String)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            // The root itself failing to even list is a hard error — real
            // bug, found live: a network mount (SMB) that dropped mid-scan
            // made this look identical to "the root is just empty now",
            // and scan_roots' own reconciliation then deleted every
            // already-known entry under it, wiping the whole local library
            // even though the real files were untouched on the still-alive
            // remote share. A nested subdirectory failing (permissions, a
            // broken symlink target) is still just skipped — only the walk's
            // very first directory (the root itself) gets this treatment.
            Err(e) if first => return Err(e),
            Err(_) => continue, // unreadable subdir: skip, don't fail the scan
        };
        first = false;
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
                if let Some(p) = progress {
                    p.tick_discovering();
                }
                out.push((path, relative));
            }
        }
    }
    Ok(out)
}
