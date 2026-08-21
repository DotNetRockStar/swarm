//! One-shot bulk scrape job: walk entries missing a scrape result, resolve
//! them against TMDb (movies/episodes) or MusicBrainz+Cover Art
//! Archive+Wikimedia (tracks), and write the results back to the library.
//!
//! Inherited scar tissue from Batocera.Drone's scrape jobs: entries are
//! processed one-at-a-time so a single 404 fails one title instead of the
//! whole run (`NotFound` vs `Unavailable` — only the latter leaves the entry
//! unscraped for a retry); music is matched **per (artist, album)**, not per
//! track, both because that's the natural unit for MusicBrainz releases and
//! because it turns N tracks into one MusicBrainz call under its 1 req/s
//! throttle; artwork downloads are best-effort and never affect the
//! matched/not-found/failed counts. Concurrent runs are the caller's
//! responsibility to prevent (see `ServerCore`'s scrape guard).

use crate::roots::SharedRootResolver;
use crate::scrape::artwork;
use crate::scrape::coverart::{CoverArtClient, CoverArtError};
use crate::scrape::musicbrainz::{MbError, MusicBrainzClient};
use crate::scrape::tmdb::{ScrapedVideo, TmdbClient, TmdbError, TmdbOverride};
use crate::scrape::wikimedia::WikimediaClient;
use crate::store::{ArtworkKind, CastMember, EntryRecord, Library};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use swarm_core::peer::MediaKind;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Default)]
pub struct ScrapeConfig {
    /// Movie/episode scraping is skipped entirely (counted as `skipped`)
    /// when no key is configured — TMDb requires one, unlike the keyless
    /// music sources.
    pub tmdb_api_key: Option<String>,
    /// Overrides for the real service URLs — used by tests to point at a
    /// local mock, and available to self-hosters who want to run behind a
    /// mirror. `None` means the real public endpoint.
    pub tmdb_api_base: Option<String>,
    pub tmdb_image_base: Option<String>,
    pub musicbrainz_base: Option<String>,
    pub coverart_base: Option<String>,
    pub wikimedia_base: Option<String>,
}

/// One entry (or, for a music `(artist, album)` group, one representative
/// track) that didn't come back `matched`, with enough detail to actually
/// act on it — the aggregate counts alone can't distinguish "this title
/// genuinely isn't on TMDb" from "every request failed because of a bad
/// API key," which is exactly the ambiguity that made a real 401-vs-key-
/// format bug (see `tmdb::is_v4_read_access_token`) invisible until someone
/// went looking at debug logs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ScrapeIssue {
    pub entry_key: String,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BulkScrapeReport {
    pub matched: u64,
    pub not_found: u64,
    pub failed: u64,
    pub skipped: u64,
    pub issues: Vec<ScrapeIssue>,
}

/// One entry's outcome, for [`ScrapeProgressEvent`] — a superset of what
/// [`BulkScrapeReport`]'s counters track (adds `Skipped` so `processed`
/// reliably reaches `total` even for entries the report only ever counts in
/// bulk, e.g. every video when no TMDb key is configured).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrapeOutcome {
    Matched,
    NotFound,
    Failed,
    Skipped,
}

/// Live per-entry progress, emitted as each entry (or, for a music album
/// group, each track within it) finishes — carries the freshly-written
/// `scraped_title`/`genres`/`cast` directly (not just a "something changed"
/// signal) so a listener can patch its own rendering immediately, with no
/// extra round trip back to the library for the data that just changed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ScrapeProgressEvent {
    pub entry_key: String,
    /// The entry's original (path-derived) title — stable identification
    /// even before/without a scrape match, unlike `scraped_title` below.
    pub title: String,
    pub processed: u64,
    pub total: u64,
    pub outcome: ScrapeOutcome,
    /// Set for `NotFound`/`Failed` — the same human-readable reason
    /// [`ScrapeIssue`] carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Set for a `Matched` movie/episode only — tracks never get a
    /// `scraped_title` (see `scrape_one_album_group`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scraped_title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cast: Vec<CastMember>,
}

/// Optional live progress side-channel for [`run_bulk_scrape`] — a plain
/// `mpsc` sender, not a Tauri type, so this crate stays usable by the
/// headless daemon with zero UI dependency; the GUI layer is what turns
/// received events into `app.emit(...)` calls. `total` is fixed once at
/// construction (the entry count already known before the loop starts);
/// `processed` counts up via an atomic since [`ScrapeProgress::emit`] is
/// called from sequential `.await` points, not concurrently, but a plain
/// `Cell` wouldn't be `Send` across them.
pub struct ScrapeProgress {
    sender: UnboundedSender<ScrapeProgressEvent>,
    total: u64,
    processed: AtomicU64,
}

impl ScrapeProgress {
    fn new(sender: UnboundedSender<ScrapeProgressEvent>, total: u64) -> Self {
        Self { sender, total, processed: AtomicU64::new(0) }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit(
        &self,
        entry_key: &str,
        title: &str,
        outcome: ScrapeOutcome,
        reason: Option<String>,
        scraped_title: Option<String>,
        genres: Vec<String>,
        cast: Vec<CastMember>,
    ) {
        let processed = self.processed.fetch_add(1, Ordering::Relaxed) + 1;
        // A closed receiver (nothing currently listening, or the listener
        // was dropped) just means no one's watching this run — never fail
        // the scrape itself over it.
        let _ = self.sender.send(ScrapeProgressEvent {
            entry_key: entry_key.to_string(),
            title: title.to_string(),
            processed,
            total: self.total,
            outcome,
            reason,
            scraped_title,
            genres,
            cast,
        });
    }

    fn matched(&self, entry_key: &str, title: &str, scraped_title: Option<String>, genres: Vec<String>, cast: Vec<CastMember>) {
        self.emit(entry_key, title, ScrapeOutcome::Matched, None, scraped_title, genres, cast);
    }

    fn issue(&self, entry_key: &str, title: &str, outcome: ScrapeOutcome, reason: String) {
        self.emit(entry_key, title, outcome, Some(reason), None, vec![], vec![]);
    }

    fn skipped(&self, entry_key: &str, title: &str) {
        self.emit(entry_key, title, ScrapeOutcome::Skipped, None, None, vec![], vec![]);
    }
}

pub async fn run_bulk_scrape(
    library: &Library,
    roots: &SharedRootResolver,
    config: &ScrapeConfig,
    cancel: &AtomicBool,
    progress_tx: Option<UnboundedSender<ScrapeProgressEvent>>,
    force: bool,
) -> sqlx::Result<BulkScrapeReport> {
    // `force` re-scrapes everything, overwriting whatever's already there —
    // `scrape_videos`/`scrape_tracks` below always overwrite unconditionally
    // per entry regardless of prior state, so simply widening which entries
    // are handed in is the whole difference; no separate "overwrite" branch
    // needed in the per-entry scraping logic itself.
    let entries = if force { library.list().await? } else { library.missing_scrape().await? };
    let (videos, tracks): (Vec<EntryRecord>, Vec<EntryRecord>) =
        entries.into_iter().partition(|e| matches!(e.kind, MediaKind::Movie | MediaKind::Episode));
    let progress = progress_tx.map(|sender| ScrapeProgress::new(sender, (videos.len() + tracks.len()) as u64));

    // Movies/episodes (TMDb) and music (MusicBrainz/Cover Art Archive/
    // Wikimedia) hit entirely independent external services with
    // independent rate limits, so run them concurrently rather than
    // sequentially — one waiting on a TMDb response no longer blocks the
    // other's MusicBrainz request (and vice versa). `tokio::join!` (not
    // `spawn`) is enough for this: these are I/O-bound, so cooperative
    // concurrency on one task already lets both make progress while the
    // other's request is in flight, and it avoids the `Send`/`'static`
    // bounds `spawn` would force onto every borrowed argument here. A
    // shared `&mut BulkScrapeReport` isn't expressible across two
    // concurrently-polled futures, so each accumulates its own and the two
    // are merged once both finish; `scrape_tracks` no longer needs the
    // explicit pre-flight cancel check the old sequential call had — its
    // own loop already checks `cancel` before every iteration, so a
    // cancellation set during the video half still stops the track half on
    // its very first iteration either way.
    let mut video_report = BulkScrapeReport::default();
    let mut track_report = BulkScrapeReport::default();
    let (video_result, track_result) = tokio::join!(
        scrape_videos(library, roots, config, &videos, cancel, &mut video_report, progress.as_ref()),
        scrape_tracks(library, roots, config, &tracks, cancel, &mut track_report, progress.as_ref()),
    );
    video_result?;
    track_result?;

    Ok(BulkScrapeReport {
        matched: video_report.matched + track_report.matched,
        not_found: video_report.not_found + track_report.not_found,
        failed: video_report.failed + track_report.failed,
        skipped: video_report.skipped + track_report.skipped,
        issues: [video_report.issues, track_report.issues].concat(),
    })
}

/// Scene-release tags that routinely survive `classify::clean_title` (which
/// only strips bracketed groups and a bare year — see its module doc) but
/// choke TMDb's search matcher when left in the query: confirmed live
/// against the real API that `"10 Cloverfield Lane 1080p BluRay x264"`
/// returns zero results while `"10 Cloverfield Lane"` matches immediately.
/// Not a stored-title change — this only affects the string sent to TMDb
/// search, so it can't touch anything `classify.rs`'s own tests already
/// cover. Truncates the query at the first case-insensitive occurrence of
/// any of these (a token boundary — not a raw substring match, so a real
/// title that happens to contain one of these strings as part of a real
/// word, e.g. wouldn't false-positive on a partial match inside a longer
/// word) and trims what's left.
const SEARCH_QUERY_NOISE_TOKENS: &[&str] = &[
    "480p", "720p", "1080p", "2160p", "4k", "bluray", "blu-ray", "webrip", "web-dl", "webdl",
    "hdtv", "dvdrip", "brrip", "bdrip", "x264", "x265", "h264", "h265", "hevc", "avc", "xvid",
    "10bit", "8bit", "ddp5", "ddp", "dts", "aac", "ac3", "atmos",
    // Scene-release qualifier tags — describe the *release*, not the movie,
    // but (unlike the codec/resolution tags above) don't reliably follow
    // right after the title: "Proper" can sit between the title and the
    // resolution tag (e.g. "28 Years Later Proper 1080p..."), which used to
    // survive into the search query and broke an otherwise-correct match.
    "proper", "repack", "rerip", "internal", "limited", "unrated", "extended", "remastered",
    "theatrical", "directors.cut", "uncut", "retail", "readnfo", "nfofix", "subbed", "dubbed",
    // Streaming-service source tags, same reasoning.
    "amzn", "nf", "dsnp", "hulu", "hmax", "atvp", "pcok",
];

/// See [`SEARCH_QUERY_NOISE_TOKENS`]. Splits on whitespace, drops every
/// token from the first noise token onward, and rejoins — cheap and
/// dependency-free, matching this module's existing hand-rolled style.
fn search_query_for(title: &str) -> String {
    let mut kept = Vec::new();
    for word in title.split_whitespace() {
        let lower = word.to_lowercase();
        if SEARCH_QUERY_NOISE_TOKENS.iter().any(|tag| lower == *tag || lower.trim_end_matches(['.', ',']) == *tag) {
            break;
        }
        kept.push(word);
    }
    if kept.is_empty() {
        title.to_string()
    } else {
        kept.join(" ")
    }
}

async fn scrape_videos(
    library: &Library,
    roots: &SharedRootResolver,
    config: &ScrapeConfig,
    entries: &[EntryRecord],
    cancel: &AtomicBool,
    report: &mut BulkScrapeReport,
    progress: Option<&ScrapeProgress>,
) -> sqlx::Result<()> {
    let Some(api_key) = &config.tmdb_api_key else {
        report.skipped += entries.len() as u64;
        if let Some(p) = progress {
            for entry in entries {
                p.skipped(&entry.entry_key, &entry.title);
            }
        }
        return Ok(());
    };
    let tmdb = match (&config.tmdb_api_base, &config.tmdb_image_base) {
        (Some(api_base), Some(image_base)) => TmdbClient::with_base_urls(api_key.clone(), api_base, image_base),
        _ => TmdbClient::new(api_key.clone()),
    };
    // One TMDb TV lookup per show, shared across every episode entry —
    // avoids N calls for an N-episode season.
    let mut tv_cache: HashMap<String, Result<ScrapedVideo, TmdbError>> = HashMap::new();

    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let outcome = match entry.kind {
            MediaKind::Movie => tmdb.search_and_fetch_movie(&search_query_for(&entry.title), entry.year).await,
            MediaKind::Episode => {
                let raw_query = entry.show_title.clone().unwrap_or_else(|| entry.title.clone());
                let query = search_query_for(&raw_query);
                let key = query.to_lowercase();
                if !tv_cache.contains_key(&key) {
                    let result = tmdb.search_and_fetch_tv(&query).await;
                    tv_cache.insert(key.clone(), result);
                }
                tv_cache.get(&key).expect("just inserted").clone()
            }
            MediaKind::Track => unreachable!("videos partition excludes tracks"),
        };
        match outcome {
            Ok(scraped) => {
                library.set_scrape_result(&entry.entry_key, Some(&scraped.title), &scraped.genres, &scraped.cast).await?;
                if let Some(overview) = &scraped.overview {
                    library.set_overview(&entry.entry_key, overview).await?;
                }
                if let Some(certification) = &scraped.certification {
                    library.set_rating(&entry.entry_key, certification).await?;
                }
                if let Some(url) = &scraped.poster_url {
                    save_video_artwork(library, roots, entry, ArtworkKind::Poster, "poster", url).await;
                }
                if let Some(url) = &scraped.backdrop_url {
                    save_video_artwork(library, roots, entry, ArtworkKind::Backdrop, "backdrop", url).await;
                }
                report.matched += 1;
                if let Some(p) = progress {
                    p.matched(&entry.entry_key, &entry.title, Some(scraped.title.clone()), scraped.genres.clone(), scraped.cast.clone());
                }
            }
            Err(TmdbError::NotFound) => {
                library.set_scrape_result(&entry.entry_key, None, &[], &[]).await?;
                report.not_found += 1;
                let reason = "no match found on TMDb".to_string();
                report.issues.push(ScrapeIssue { entry_key: entry.entry_key.clone(), title: entry.title.clone(), reason: reason.clone() });
                if let Some(p) = progress {
                    p.issue(&entry.entry_key, &entry.title, ScrapeOutcome::NotFound, reason);
                }
            }
            Err(TmdbError::Unavailable(reason)) => {
                tracing::warn!(entry = %entry.entry_key, %reason, "tmdb unavailable, will retry next run");
                report.failed += 1;
                report.issues.push(ScrapeIssue { entry_key: entry.entry_key.clone(), title: entry.title.clone(), reason: reason.clone() });
                if let Some(p) = progress {
                    p.issue(&entry.entry_key, &entry.title, ScrapeOutcome::Failed, reason);
                }
            }
        }
    }
    Ok(())
}

async fn save_video_artwork(
    library: &Library,
    roots: &SharedRootResolver,
    entry: &EntryRecord,
    kind: ArtworkKind,
    label: &str,
    url: &str,
) {
    let Ok(bytes) = download_bytes(url).await else { return };
    let filename = format!("{}-tmdb-{label}.jpg", artwork::sanitize_stem(artwork::file_stem(&entry.relative_path)));
    if let Ok(relative) = artwork::save_artwork(roots, &entry.relative_path, &filename, &bytes).await {
        let _ = library.set_artwork(&entry.entry_key, kind, &relative).await;
    }
}

/// The three music-scraper clients bundled together purely to keep
/// [`scrape_one_album_group`]'s argument count sane — always constructed
/// and passed as one unit.
struct MusicScrapers {
    mb: MusicBrainzClient,
    coverart: CoverArtClient,
    wikimedia: WikimediaClient,
}

impl MusicScrapers {
    fn from_config(config: &ScrapeConfig) -> Self {
        Self {
            mb: config.musicbrainz_base.as_deref().map_or_else(MusicBrainzClient::new, MusicBrainzClient::with_base_url),
            coverart: config.coverart_base.as_deref().map_or_else(CoverArtClient::new, CoverArtClient::with_base_url),
            wikimedia: config.wikimedia_base.as_deref().map_or_else(WikimediaClient::new, WikimediaClient::with_base_url),
        }
    }
}

async fn scrape_tracks(
    library: &Library,
    roots: &SharedRootResolver,
    config: &ScrapeConfig,
    entries: &[EntryRecord],
    cancel: &AtomicBool,
    report: &mut BulkScrapeReport,
    progress: Option<&ScrapeProgress>,
) -> sqlx::Result<()> {
    let scrapers = MusicScrapers::from_config(config);

    let mut groups: HashMap<(String, String), Vec<&EntryRecord>> = HashMap::new();
    for entry in entries {
        match (&entry.artist, &entry.album) {
            (Some(artist), Some(album)) if !artist.is_empty() && !album.is_empty() => {
                groups.entry((artist.clone(), album.clone())).or_default().push(entry);
            }
            _ => {
                report.skipped += 1;
                if let Some(p) = progress {
                    p.skipped(&entry.entry_key, &entry.title);
                }
            }
        }
    }

    for ((artist, album), group) in groups {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        scrape_one_album_group(library, roots, &scrapers, &artist, &album, &group, report, progress).await?;
    }
    Ok(())
}

/// Cleans a classify()-derived album name before sending it to
/// MusicBrainz's search — real libraries embed a release year, the
/// artist's own name, and/or catalog-number/edition metadata directly in
/// the album folder name (`"2004 - No Silence (AVTCD-95770)"`,
/// `"Cosmic Gate - The Drums (7243 886923 2 8)"`), none of which
/// MusicBrainz's search can match against as-is. Confirmed live against the
/// real MusicBrainz API for both patterns: the raw folder names found zero
/// results; the cleaned forms ("No Silence", "The Drums") found the correct
/// release, in the second case only after *also* stripping the redundant
/// leading artist name. Strips, in order: a leading `NNNN - ` year prefix,
/// a leading `"<artist> - "` prefix matching `artist` (case-insensitive —
/// real libraries duplicate the artist name into the album folder more
/// often once the year prefix is already gone), then repeatedly strips
/// trailing `(...)`/`[...]` groups (catalog numbers, edition/format tags
/// like `[2CD]` often stack more than one). Falls back to the original text
/// if cleaning would leave nothing — same "never send an empty query"
/// guard as [search_query_for].
fn search_query_for_album(artist: &str, album: &str) -> String {
    let mut s = album.trim();
    let bytes = s.as_bytes();
    if bytes.len() > 4 && bytes[..4].iter().all(u8::is_ascii_digit) {
        if let Some(after_dash) = s[4..].trim_start().strip_prefix('-') {
            s = after_dash.trim_start();
        }
    }
    if s.len() > artist.len() && s[..artist.len()].eq_ignore_ascii_case(artist) {
        if let Some(after_dash) = s[artist.len()..].trim_start().strip_prefix('-') {
            s = after_dash.trim_start();
        }
    }
    loop {
        let trimmed = s.trim_end();
        if let Some(rest) = trimmed.strip_suffix(')') {
            if let Some(open) = rest.rfind('(') {
                s = rest[..open].trim_end();
                continue;
            }
        }
        if let Some(rest) = trimmed.strip_suffix(']') {
            if let Some(open) = rest.rfind('[') {
                s = rest[..open].trim_end();
                continue;
            }
        }
        s = trimmed;
        break;
    }
    let cleaned = s.trim_end_matches('-').trim();
    if cleaned.is_empty() { album.to_string() } else { cleaned.to_string() }
}

/// One (artist, album) group's worth of MusicBrainz + Cover Art Archive +
/// Wikimedia work — release-level data applied to every track in the group
/// (see the module docs on why this is per-album, not per-track). Shared by
/// the bulk loop above and [`scrape_one_track`]'s pinpoint path so a single
/// manually-triggered rescrape of one track re-syncs its whole album, same
/// as a bulk run would.
#[allow(clippy::too_many_arguments)]
async fn scrape_one_album_group(
    library: &Library,
    roots: &SharedRootResolver,
    scrapers: &MusicScrapers,
    artist: &str,
    album: &str,
    group: &[&EntryRecord],
    report: &mut BulkScrapeReport,
    progress: Option<&ScrapeProgress>,
) -> sqlx::Result<()> {
    let MusicScrapers { mb, coverart, wikimedia } = scrapers;
    match mb.search_release(artist, &search_query_for_album(artist, album)).await {
        Err(MbError::NotFound) => {
            for track in group {
                library.set_scrape_result(&track.entry_key, None, &[], &[]).await?;
                let reason = format!("no match found on MusicBrainz for \"{artist} \u{2013} {album}\"");
                report.issues.push(ScrapeIssue { entry_key: track.entry_key.clone(), title: track.title.clone(), reason: reason.clone() });
                if let Some(p) = progress {
                    p.issue(&track.entry_key, &track.title, ScrapeOutcome::NotFound, reason.clone());
                }
            }
            report.not_found += group.len() as u64;
        }
        Err(MbError::Unavailable(reason)) => {
            tracing::warn!(%artist, %album, %reason, "musicbrainz unavailable, will retry next run");
            for track in group {
                report.issues.push(ScrapeIssue { entry_key: track.entry_key.clone(), title: track.title.clone(), reason: reason.clone() });
                if let Some(p) = progress {
                    p.issue(&track.entry_key, &track.title, ScrapeOutcome::Failed, reason.clone());
                }
            }
            report.failed += group.len() as u64;
        }
        Ok(release_mbid) => {
            let details = mb.release_lookup(&release_mbid).await.ok();
            let genres = details.as_ref().map(|d| d.genres.clone()).unwrap_or_default();
            for track in group {
                library.set_scrape_result(&track.entry_key, None, &genres, &[]).await?;
            }
            report.matched += group.len() as u64;

            if let Some(first) = group.first() {
                if let Ok(cover) = coverart.front_cover(&release_mbid).await {
                    if let Ok(relative) =
                        artwork::save_artwork(roots, &first.relative_path, "album-cover.jpg", &cover).await
                    {
                        for track in group {
                            let _ = library.set_artwork(&track.entry_key, ArtworkKind::Cover, &relative).await;
                        }
                    }
                }
                if let Some(artist_mbid) = details.as_ref().and_then(|d| d.artist_mbid.as_deref()) {
                    if let Some(bytes) = fetch_artist_photo(mb, wikimedia, artist_mbid).await {
                        if let Ok(relative) =
                            artwork::save_artwork(roots, &first.relative_path, "artist-photo.jpg", &bytes).await
                        {
                            for track in group {
                                let _ = library.set_artwork(&track.entry_key, ArtworkKind::ArtistPhoto, &relative).await;
                            }
                        }
                    }
                }
            }

            // Emitted after every write above (metadata + best-effort
            // artwork) completes, so a listener that reacts to this event by
            // re-fetching artwork bytes actually finds something there.
            if let Some(p) = progress {
                for track in group {
                    p.matched(&track.entry_key, &track.title, None, genres.clone(), vec![]);
                }
            }
        }
    }
    Ok(())
}

/// Pinpoint (single-entry) video scrape — bypasses `missing_scrape()`
/// entirely (the caller already has the specific `EntryRecord` in hand, via
/// `Library::get`), so this succeeds even on an already-scraped entry: the
/// whole point is correcting a wrong bulk match. `tmdb_override`, when
/// given, skips search and fetches that exact TMDb id/URL directly.
pub async fn scrape_one_video(
    library: &Library,
    roots: &SharedRootResolver,
    config: &ScrapeConfig,
    entry: &EntryRecord,
    tmdb_override: Option<TmdbOverride>,
) -> Result<ScrapedVideo, ScrapeOneError> {
    let api_key = config.tmdb_api_key.as_ref().ok_or(ScrapeOneError::NoApiKey)?;
    let tmdb = match (&config.tmdb_api_base, &config.tmdb_image_base) {
        (Some(api_base), Some(image_base)) => TmdbClient::with_base_urls(api_key.clone(), api_base, image_base),
        _ => TmdbClient::new(api_key.clone()),
    };
    let media_type = match entry.kind {
        MediaKind::Movie => "movie",
        MediaKind::Episode => "tv",
        MediaKind::Track => return Err(ScrapeOneError::WrongKind),
    };
    let scraped = match tmdb_override {
        Some(over) => {
            let id = over.resolve_id()?;
            tmdb.details_by_id(id, media_type).await?
        }
        None => match entry.kind {
            MediaKind::Movie => tmdb.search_and_fetch_movie(&search_query_for(&entry.title), entry.year).await?,
            MediaKind::Episode => {
                let raw_query = entry.show_title.clone().unwrap_or_else(|| entry.title.clone());
                tmdb.search_and_fetch_tv(&search_query_for(&raw_query)).await?
            }
            MediaKind::Track => unreachable!("checked above"),
        },
    };
    library.set_scrape_result(&entry.entry_key, Some(&scraped.title), &scraped.genres, &scraped.cast).await?;
    if let Some(overview) = &scraped.overview {
        library.set_overview(&entry.entry_key, overview).await?;
    }
    if let Some(certification) = &scraped.certification {
        library.set_rating(&entry.entry_key, certification).await?;
    }
    if let Some(url) = &scraped.poster_url {
        save_video_artwork(library, roots, entry, ArtworkKind::Poster, "poster", url).await;
    }
    if let Some(url) = &scraped.backdrop_url {
        save_video_artwork(library, roots, entry, ArtworkKind::Backdrop, "backdrop", url).await;
    }
    Ok(scraped)
}

/// Pinpoint (single-entry) music scrape: re-syncs the *whole* (artist,
/// album) group `entry` belongs to, same as bulk would — see
/// `scrape_one_album_group`'s doc comment for why one track can't be
/// rescraped in isolation without leaving its siblings stale.
pub async fn scrape_one_track(
    library: &Library,
    roots: &SharedRootResolver,
    config: &ScrapeConfig,
    entry: &EntryRecord,
) -> Result<BulkScrapeReport, ScrapeOneError> {
    let artist = entry.artist.as_deref().filter(|a| !a.is_empty());
    let album = entry.album.as_deref().filter(|a| !a.is_empty());
    let (Some(artist), Some(album)) = (artist, album) else {
        return Err(ScrapeOneError::MissingMusicMetadata);
    };
    let siblings = library.entries_by_artist_album(artist, album).await?;
    let group: Vec<&EntryRecord> = siblings.iter().collect();
    let scrapers = MusicScrapers::from_config(config);
    let mut report = BulkScrapeReport::default();
    scrape_one_album_group(library, roots, &scrapers, artist, album, &group, &mut report, None).await?;
    Ok(report)
}

#[derive(Debug, thiserror::Error)]
pub enum ScrapeOneError {
    #[error("no TMDb API key configured")]
    NoApiKey,
    #[error("this entry is music, not a movie or episode")]
    WrongKind,
    #[error(transparent)]
    Tmdb(#[from] TmdbError),
    #[error(transparent)]
    Override(#[from] crate::scrape::tmdb::TmdbOverrideError),
    #[error("library error: {0}")]
    Store(#[from] sqlx::Error),
    #[error("this entry has no artist/album metadata to search MusicBrainz with")]
    MissingMusicMetadata,
    #[error(transparent)]
    MusicBrainz(#[from] MbError),
}

/// Best-effort artist photo: MusicBrainz artist -> Commons file relation ->
/// Wikimedia URL resolution -> download. Any step failing just means no
/// photo this run; it never affects match/not-found/failed accounting.
async fn fetch_artist_photo(mb: &MusicBrainzClient, wikimedia: &WikimediaClient, artist_mbid: &str) -> Option<Vec<u8>> {
    let artist = mb.artist_lookup(artist_mbid).await.ok()?;
    let commons_file = artist.commons_file?;
    let url = wikimedia.resolve_file_url(&commons_file).await.ok()?;
    wikimedia.download(&url).await.ok()
}

async fn download_bytes(url: &str) -> Result<Vec<u8>, CoverArtError> {
    let response = reqwest::get(url).await.map_err(|e| CoverArtError::Unavailable(e.to_string()))?;
    if !response.status().is_success() {
        return Err(CoverArtError::Unavailable(format!("download returned {}", response.status())));
    }
    response.bytes().await.map(|b| b.to_vec()).map_err(|e| CoverArtError::Unavailable(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::scan_root;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use std::sync::atomic::AtomicBool;

    fn resolver(root: &std::path::Path) -> SharedRootResolver {
        SharedRootResolver::new(crate::roots::RootResolver::single(root.to_path_buf()))
    }

    #[test]
    fn search_query_strips_release_tags_confirmed_to_break_tmdb_search() {
        // Both queries below are real filenames that returned zero TMDb
        // results with the noise attached and matched immediately once
        // stripped (verified live against the real API, not assumed).
        assert_eq!(search_query_for("10 Cloverfield Lane 1080p BluRay x264"), "10 Cloverfield Lane");
        assert_eq!(
            search_query_for("28 Days Later 1080p BluRay DDP5 1 x265 10bit-GalaxyRG265"),
            "28 Days Later"
        );
        // A scene-release qualifier tag ("Proper") sitting between the title
        // and the resolution tag used to survive into the query and broke
        // an otherwise-correct match — confirmed live: TMDb returns zero
        // results for "28 Years Later Proper" but one correct result (id
        // 1100988) for "28 Years Later".
        assert_eq!(
            search_query_for("28 Years Later Proper 1080p WEB-DL DDP5 1 x265-NeoNoir"),
            "28 Years Later"
        );
    }

    #[test]
    fn search_query_is_case_insensitive_and_matches_a_whole_token_only() {
        assert_eq!(search_query_for("The Matrix WEBRip"), "The Matrix");
        // "4k" is a noise token, but "4kids" (a hypothetical real title word)
        // must not be truncated on a partial match.
        assert_eq!(search_query_for("4kids and Counting"), "4kids and Counting");
    }

    #[test]
    fn search_query_with_no_noise_tokens_is_unchanged() {
        assert_eq!(search_query_for("Heat"), "Heat");
        assert_eq!(search_query_for("The Dark Knight"), "The Dark Knight");
    }

    #[test]
    fn search_query_that_is_entirely_noise_falls_back_to_the_original_title() {
        // Never send TMDb an empty query — a title so noisy every token
        // matches should degrade to searching the raw title, not "".
        assert_eq!(search_query_for("1080p x264"), "1080p x264");
    }

    // --- search_query_for_album: real-library-confirmed album name cleanup ---
    // Real bug, confirmed live against the real MusicBrainz API: the raw
    // folder name "2004 - No Silence (AVTCD-95770)" found zero results;
    // "No Silence" found the correct release at the top score.

    #[test]
    fn album_query_strips_leading_year_and_trailing_catalog_code() {
        assert_eq!(search_query_for_album("ATB", "2004 - No Silence (AVTCD-95770)"), "No Silence");
        assert_eq!(search_query_for_album("ATB", "2003 - Addicted To Music (BANGCD029)"), "Addicted To Music");
    }

    #[test]
    fn album_query_strips_a_redundant_leading_artist_name() {
        // Real bug, confirmed live: the year-only-stripped form ("Cosmic
        // Gate - The Drums") still found zero MusicBrainz results; only
        // stripping the redundant artist prefix too ("The Drums") found
        // the correct release.
        assert_eq!(
            search_query_for_album("Cosmic Gate", "1999 - Cosmic Gate - The Drums (7243 886923 2 8)"),
            "The Drums"
        );
    }

    #[test]
    fn album_query_strips_multiple_trailing_bracket_groups() {
        assert_eq!(
            search_query_for_album("Kyau & Albert", "2009 - Kyau & Albert - Best Of 2002-2009 (EUPH100CD) [CD]"),
            "Best Of 2002-2009"
        );
    }

    #[test]
    fn album_query_with_no_noise_is_unchanged() {
        assert_eq!(search_query_for_album("Pink Floyd", "The Wall"), "The Wall");
    }

    #[test]
    fn album_query_that_is_entirely_noise_falls_back_to_the_original() {
        assert_eq!(search_query_for_album("ATB", "2004 - (CATALOG-1)"), "2004 - (CATALOG-1)");
    }

    fn fixture_dirs(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!("swarm-scrape-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        (base.join("media"), base.join("library.sqlite"))
    }

    #[tokio::test]
    async fn no_tmdb_key_skips_videos_without_touching_tracks() {
        let (root, db_path) = fixture_dirs("no-key");
        std::fs::create_dir_all(root.join("movies/Heat (1995)")).unwrap();
        std::fs::write(root.join("movies/Heat (1995)/Heat.1995.mkv"), vec![0u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();

        let report =
            run_bulk_scrape(&library, &resolver(&root), &ScrapeConfig::default(), &AtomicBool::new(false), None, false).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 0, not_found: 0, failed: 0, skipped: 1, issues: vec![] });
        // Skipped entries stay unscraped so a later run (with a key) retries them.
        assert_eq!(library.missing_scrape().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn end_to_end_movie_scrape_writes_title_genres_and_poster() {
        let (root, db_path) = fixture_dirs("movie-e2e");
        std::fs::create_dir_all(root.join("movies/Heat (1995)")).unwrap();
        std::fs::write(root.join("movies/Heat (1995)/Heat.1995.mkv"), vec![0u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();

        // Bind first so the poster path can embed the mock server's own address.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/search/movie", get(|| async { Json(json!({"results": [{"id": 1}]})) }))
            .route(
                "/movie/1",
                get(|| async {
                    Json(json!({
                        "title": "Heat", "genres": [{"name": "Crime"}], "poster_path": "/p.jpg",
                        "overview": "A group of professional bank robbers start to feel the heat from police.",
                        "credits": {"cast": [{"name": "Al Pacino", "character": "Vincent Hanna"}]}
                    }))
                }),
            )
            .route("/img/w342/p.jpg", get(|| async { [9u8, 9, 9] }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let config = ScrapeConfig {
            tmdb_api_key: Some("key".into()),
            tmdb_api_base: Some(format!("http://{addr}")),
            tmdb_image_base: Some(format!("http://{addr}/img")),
            ..Default::default()
        };
        let report = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false), None, false).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 1, not_found: 0, failed: 0, skipped: 0, issues: vec![] });

        let entry = &library.list().await.unwrap()[0];
        assert_eq!(entry.scraped_title.as_deref(), Some("Heat"));
        assert_eq!(entry.genres, vec!["Crime"]);
        assert_eq!(entry.cast.len(), 1);
        assert_eq!(entry.cast[0].name, "Al Pacino");
        assert_eq!(entry.cast[0].character.as_deref(), Some("Vincent Hanna"));
        assert_eq!(entry.overview.as_deref(), Some("A group of professional bank robbers start to feel the heat from police."));
        assert_eq!(entry.artwork_version, 1);
        let (art_path, art_version) = library.artwork(&entry.entry_key, ArtworkKind::Poster).await.unwrap().unwrap();
        assert_eq!(art_version, 1);
        assert_eq!(std::fs::read(root.join(&art_path)).unwrap(), vec![9, 9, 9]);
        assert!(library.missing_scrape().await.unwrap().is_empty());
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn pinpoint_scrape_overwrites_an_already_matched_entry() {
        let (root, db_path) = fixture_dirs("pinpoint-overwrite");
        std::fs::create_dir_all(root.join("movies/Heat (1995)")).unwrap();
        std::fs::write(root.join("movies/Heat (1995)/Heat.1995.mkv"), vec![0u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/search/movie", get(|| async { Json(json!({"results": [{"id": 1}]})) }))
            .route("/movie/1", get(|| async { Json(json!({"title": "Wrong Match", "genres": []})) }))
            .route("/movie/2", get(|| async { Json(json!({"title": "Heat", "genres": [{"name": "Crime"}]})) }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let config =
            ScrapeConfig { tmdb_api_key: Some("key".into()), tmdb_api_base: Some(format!("http://{addr}")), tmdb_image_base: Some(format!("http://{addr}")), ..Default::default() };

        // First pass: bulk scrape matches the wrong title (simulating a bad match).
        run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false), None, false).await.unwrap();
        let entry = library.list().await.unwrap().into_iter().next().unwrap();
        assert_eq!(entry.scraped_title.as_deref(), Some("Wrong Match"));

        // Pinpoint rescrape with a manual override to the correct id must
        // succeed even though the entry is already "processed" per
        // missing_scrape, and must overwrite the previous (wrong) result.
        assert!(library.missing_scrape().await.unwrap().is_empty());
        let scraped =
            scrape_one_video(&library, &resolver(&root), &config, &entry, Some(TmdbOverride::Id(2))).await.unwrap();
        assert_eq!(scraped.title, "Heat");
        let updated = library.get(&entry.entry_key).await.unwrap().unwrap();
        assert_eq!(updated.scraped_title.as_deref(), Some("Heat"));
        assert_eq!(updated.genres, vec!["Crime"]);
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn bulk_scrape_force_rescrapes_already_processed_entries() {
        let (root, db_path) = fixture_dirs("bulk-force-rescrape");
        std::fs::create_dir_all(root.join("movies/Heat (1995)")).unwrap();
        std::fs::write(root.join("movies/Heat (1995)/Heat.1995.mkv"), vec![0u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/search/movie", get(|| async { Json(json!({"results": [{"id": 1}]})) }))
            .route("/movie/1", get(|| async { Json(json!({"title": "Heat", "genres": []})) }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let config =
            ScrapeConfig { tmdb_api_key: Some("key".into()), tmdb_api_base: Some(format!("http://{addr}")), tmdb_image_base: Some(format!("http://{addr}")), ..Default::default() };

        let first = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false), None, false).await.unwrap();
        assert_eq!(first.matched, 1);
        assert!(library.missing_scrape().await.unwrap().is_empty(), "must be marked processed");

        // Default (force: false) must not touch an already-processed entry.
        let second = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false), None, false).await.unwrap();
        assert_eq!(second, BulkScrapeReport::default(), "nothing left to do without force");

        // force: true must re-scrape it anyway, even though it's already processed.
        let third = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false), None, true).await.unwrap();
        assert_eq!(third.matched, 1, "force must re-scrape an already-processed entry");
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn manual_tmdb_url_override_skips_search_entirely() {
        let (root, db_path) = fixture_dirs("pinpoint-url-override");
        std::fs::create_dir_all(root.join("movies/Foo (2020)")).unwrap();
        std::fs::write(root.join("movies/Foo (2020)/Foo.2020.mkv"), vec![0u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();
        let entry = library.list().await.unwrap().into_iter().next().unwrap();

        // No /search/* route registered at all — a request there would 404
        // and fail the test, proving the override path never calls search.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new().route("/movie/42", get(|| async { Json(json!({"title": "Direct Hit"})) }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let config =
            ScrapeConfig { tmdb_api_key: Some("key".into()), tmdb_api_base: Some(format!("http://{addr}")), tmdb_image_base: Some(format!("http://{addr}")), ..Default::default() };

        let scraped = scrape_one_video(
            &library,
            &resolver(&root),
            &config,
            &entry,
            Some(TmdbOverride::Url("https://www.themoviedb.org/movie/42-direct-hit".to_string())),
        )
        .await
        .unwrap();
        assert_eq!(scraped.title, "Direct Hit");
    }

    #[tokio::test]
    async fn malformed_override_url_errors_cleanly_without_touching_existing_data() {
        let (root, db_path) = fixture_dirs("pinpoint-bad-url");
        std::fs::create_dir_all(root.join("movies/Foo (2020)")).unwrap();
        std::fs::write(root.join("movies/Foo (2020)/Foo.2020.mkv"), vec![0u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();
        library.set_scrape_result(&library.list().await.unwrap()[0].entry_key, Some("Existing Title"), &[], &[]).await.unwrap();
        let entry = library.list().await.unwrap().into_iter().next().unwrap();

        let config = ScrapeConfig { tmdb_api_key: Some("key".into()), ..Default::default() };
        let result = scrape_one_video(
            &library,
            &resolver(&root),
            &config,
            &entry,
            Some(TmdbOverride::Url("https://example.com/not-tmdb".into())),
        )
        .await;
        assert!(matches!(result, Err(ScrapeOneError::Override(_))));

        // Existing data must be untouched by a failed override.
        let unchanged = library.get(&entry.entry_key).await.unwrap().unwrap();
        assert_eq!(unchanged.scraped_title.as_deref(), Some("Existing Title"));
    }

    #[tokio::test]
    async fn pinpoint_track_rescrape_resyncs_the_whole_album_group() {
        let (root, db_path) = fixture_dirs("pinpoint-track");
        std::fs::create_dir_all(root.join("music/Pink Floyd/The Wall")).unwrap();
        std::fs::write(root.join("music/Pink Floyd/The Wall/01 - In The Flesh.flac"), vec![1u8; 10]).unwrap();
        std::fs::write(root.join("music/Pink Floyd/The Wall/02 - The Thin Ice.flac"), vec![2u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mb_base = format!("http://{addr}/mb");
        let router = Router::new()
            .route("/mb/release/", get(|| async { Json(json!({"releases": [{"id": "rel-1"}]})) }))
            .route("/mb/release/rel-1", get(|| async { Json(json!({"genres": [{"name": "Rock"}], "artist-credit": []})) }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let entries = library.list().await.unwrap();
        let config = ScrapeConfig { musicbrainz_base: Some(mb_base), ..Default::default() };
        let report = scrape_one_track(&library, &resolver(&root), &config, &entries[0]).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 2, not_found: 0, failed: 0, skipped: 0, issues: vec![] });
        for entry in library.list().await.unwrap() {
            assert_eq!(entry.genres, vec!["Rock"]);
        }
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn pinpoint_video_scrape_without_api_key_is_a_clean_error() {
        let (root, db_path) = fixture_dirs("pinpoint-no-key");
        std::fs::create_dir_all(root.join("movies/Foo (2020)")).unwrap();
        std::fs::write(root.join("movies/Foo (2020)/Foo.2020.mkv"), vec![0u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();
        let entry = library.list().await.unwrap().into_iter().next().unwrap();
        let result = scrape_one_video(&library, &resolver(&root), &ScrapeConfig::default(), &entry, None).await;
        assert!(matches!(result, Err(ScrapeOneError::NoApiKey)));
    }

    #[tokio::test]
    async fn not_found_movie_is_marked_processed_not_retried() {
        let (root, db_path) = fixture_dirs("movie-not-found");
        std::fs::create_dir_all(root.join("movies/Unknowable Film")).unwrap();
        std::fs::write(root.join("movies/Unknowable Film/Unknowable Film.mkv"), vec![0u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();

        let router = Router::new().route("/search/movie", get(|| async { Json(json!({"results": []})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let config = ScrapeConfig {
            tmdb_api_key: Some("key".into()),
            tmdb_api_base: Some(format!("http://{addr}")),
            tmdb_image_base: Some(format!("http://{addr}")),
            ..Default::default()
        };
        let report = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false), None, false).await.unwrap();
        assert_eq!(report.matched, 0);
        assert_eq!(report.not_found, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].title, "Unknowable Film");
        assert_eq!(report.issues[0].reason, "no match found on TMDb");
        // Processed (no match) still counts as done — must not be re-queued.
        assert!(library.missing_scrape().await.unwrap().is_empty());
        let entry = &library.list().await.unwrap()[0];
        assert_eq!(entry.scraped_title, None);
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn bulk_scrape_hits_tmdb_and_musicbrainz_concurrently_not_sequentially() {
        // Records the instant each service's *first* request lands, rather
        // than comparing total wall-clock time — MusicBrainzClient
        // self-enforces a real ~1.05s minimum interval between its own
        // requests (respecting MusicBrainz's real rate limit), which would
        // swamp any timing budget based on total duration regardless of
        // whether the two scrapes ran concurrently or not. Whether the two
        // *started* within a tight window of each other is what actually
        // distinguishes concurrent (tokio::join!) from sequential (movies
        // fully finish, including any of their own delay, before music
        // starts at all) — sequential would show a gap of at least DELAY.
        const DELAY: std::time::Duration = std::time::Duration::from_millis(250);
        let started = std::time::Instant::now();
        let tmdb_hit_at = std::sync::Arc::new(std::sync::Mutex::new(None::<std::time::Duration>));
        let mb_hit_at = std::sync::Arc::new(std::sync::Mutex::new(None::<std::time::Duration>));

        let (root, db_path) = fixture_dirs("concurrent-scrape");
        std::fs::create_dir_all(root.join("movies/Heat (1995)")).unwrap();
        std::fs::write(root.join("movies/Heat (1995)/Heat.1995.mkv"), vec![0u8; 10]).unwrap();
        std::fs::create_dir_all(root.join("music/Artist/Album")).unwrap();
        std::fs::write(root.join("music/Artist/Album/01 - Song.flac"), vec![1u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();
        assert_eq!(library.list().await.unwrap().len(), 2);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tmdb_recorder, mb_recorder) = (tmdb_hit_at.clone(), mb_hit_at.clone());
        let router = Router::new()
            .route(
                "/search/movie",
                get(move || {
                    let recorder = tmdb_recorder.clone();
                    async move {
                        *recorder.lock().unwrap() = Some(started.elapsed());
                        tokio::time::sleep(DELAY).await;
                        Json(json!({"results": [{"id": 1}]}))
                    }
                }),
            )
            .route("/movie/1", get(|| async { Json(json!({"title": "Heat", "genres": []})) }))
            .route(
                "/mb/release/",
                get(move || {
                    let recorder = mb_recorder.clone();
                    async move {
                        *recorder.lock().unwrap() = Some(started.elapsed());
                        Json(json!({"releases": [{"id": "rel-1"}]}))
                    }
                }),
            )
            .route("/mb/release/rel-1", get(|| async { Json(json!({"genres": [], "artist-credit": []})) }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let config = ScrapeConfig {
            tmdb_api_key: Some("key".into()),
            tmdb_api_base: Some(format!("http://{addr}")),
            tmdb_image_base: Some(format!("http://{addr}")),
            musicbrainz_base: Some(format!("http://{addr}/mb")),
            ..Default::default()
        };
        let report = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false), None, false).await.unwrap();

        assert_eq!(report.matched, 2, "both must still actually succeed: {report:?}");
        let tmdb_at = tmdb_hit_at.lock().unwrap().expect("TMDb search must have been hit");
        let mb_at = mb_hit_at.lock().unwrap().expect("MusicBrainz search must have been hit");
        let gap = tmdb_at.abs_diff(mb_at);
        assert!(
            gap < DELAY,
            "TMDb hit at {tmdb_at:?}, MusicBrainz hit at {mb_at:?} — a {gap:?} gap means they did not start \
             concurrently (sequential would show a gap of at least the {DELAY:?} TMDb delay, since MusicBrainz \
             would only start once the video scrape, including its delay, fully finished)"
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn music_scrape_groups_by_album_and_shares_cover_art() {
        let (root, db_path) = fixture_dirs("music-e2e");
        std::fs::create_dir_all(root.join("music/Pink Floyd/The Wall")).unwrap();
        std::fs::write(root.join("music/Pink Floyd/The Wall/01 - In The Flesh.flac"), vec![1u8; 10]).unwrap();
        std::fs::write(root.join("music/Pink Floyd/The Wall/02 - The Thin Ice.flac"), vec![2u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();
        assert_eq!(library.list().await.unwrap().len(), 2);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mb_base = format!("http://{addr}/mb");
        let ca_base = format!("http://{addr}/ca");
        let router = Router::new()
            .route("/mb/release/", get(|| async { Json(json!({"releases": [{"id": "rel-1"}]})) }))
            .route(
                "/mb/release/rel-1",
                get(|| async { Json(json!({"genres": [{"name": "Rock"}], "artist-credit": []})) }),
            )
            .route(
                "/ca/release/rel-1",
                get(move || {
                    let cover_url = format!("http://{addr}/cover.jpg");
                    async move { Json(json!({"images": [{"front": true, "image": cover_url}]})) }
                }),
            )
            .route("/cover.jpg", get(|| async { [7u8, 7, 7] }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let config = ScrapeConfig {
            musicbrainz_base: Some(mb_base),
            coverart_base: Some(ca_base),
            ..Default::default()
        };
        let report = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false), None, false).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 2, not_found: 0, failed: 0, skipped: 0, issues: vec![] });

        let entries = library.list().await.unwrap();
        for entry in &entries {
            assert_eq!(entry.genres, vec!["Rock"]);
            assert_eq!(entry.artwork_version, 1);
        }
        // Both tracks in the album point at the same physical cover file.
        let (path_a, _) = library.artwork(&entries[0].entry_key, ArtworkKind::Cover).await.unwrap().unwrap();
        let (path_b, _) = library.artwork(&entries[1].entry_key, ArtworkKind::Cover).await.unwrap().unwrap();
        assert_eq!(path_a, path_b);
        assert_eq!(std::fs::read(root.join(&path_a)).unwrap(), vec![7, 7, 7]);
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn tracks_without_artist_or_album_are_skipped() {
        let (root, db_path) = fixture_dirs("music-skip");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("orphan.mp3"), vec![0u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();

        let report =
            run_bulk_scrape(&library, &resolver(&root), &ScrapeConfig::default(), &AtomicBool::new(false), None, false).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 0, not_found: 0, failed: 0, skipped: 1, issues: vec![] });
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn progress_events_stream_one_per_entry_with_correct_data_and_running_total() {
        // 1 movie (matched) + 1 album of 2 tracks (matched) = 3 entries, so
        // exactly 3 progress events, `processed` 1..=3, `total` fixed at 3
        // throughout — this is the whole property a progress bar depends on.
        let (root, db_path) = fixture_dirs("progress-events");
        std::fs::create_dir_all(root.join("movies/Heat (1995)")).unwrap();
        std::fs::write(root.join("movies/Heat (1995)/Heat.1995.mkv"), vec![0u8; 10]).unwrap();
        std::fs::create_dir_all(root.join("music/Pink Floyd/The Wall")).unwrap();
        std::fs::write(root.join("music/Pink Floyd/The Wall/01 - In The Flesh.flac"), vec![1u8; 10]).unwrap();
        std::fs::write(root.join("music/Pink Floyd/The Wall/02 - The Thin Ice.flac"), vec![2u8; 10]).unwrap();
        let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
        scan_root(&library, &root).await.unwrap();
        assert_eq!(library.list().await.unwrap().len(), 3);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/search/movie", get(|| async { Json(json!({"results": [{"id": 1}]})) }))
            .route("/movie/1", get(|| async { Json(json!({"title": "Heat", "genres": [{"name": "Crime"}]})) }))
            .route("/mb/release/", get(|| async { Json(json!({"releases": [{"id": "rel-1"}]})) }))
            .route(
                "/mb/release/rel-1",
                get(|| async { Json(json!({"genres": [{"name": "Rock"}], "artist-credit": []})) }),
            )
            .route("/ca/release/rel-1", get(|| async { Json(json!({"images": []})) }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let config = ScrapeConfig {
            tmdb_api_key: Some("key".into()),
            tmdb_api_base: Some(format!("http://{addr}")),
            tmdb_image_base: Some(format!("http://{addr}/img")),
            musicbrainz_base: Some(format!("http://{addr}/mb")),
            coverart_base: Some(format!("http://{addr}/ca")),
            ..Default::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let report = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false), Some(tx), false).await.unwrap();
        assert_eq!(report.matched, 3);

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert_eq!(events.len(), 3, "one progress event per entry, got {events:?}");
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.processed, (i + 1) as u64, "processed must increment 1..=total in emission order");
            assert_eq!(event.total, 3, "total must stay fixed at the entry count known before the loop started");
            assert_eq!(event.outcome, ScrapeOutcome::Matched);
        }
        let movie_event = events.iter().find(|e| e.title.starts_with("Heat")).expect("movie event present");
        assert_eq!(movie_event.scraped_title.as_deref(), Some("Heat"));
        assert_eq!(movie_event.genres, vec!["Crime"]);
        let track_events: Vec<_> = events.iter().filter(|e| e.entry_key != movie_event.entry_key).collect();
        assert_eq!(track_events.len(), 2, "one event per track, not one per album group");
        for track_event in track_events {
            // Tracks never get a scraped_title (see scrape_one_album_group) —
            // only genres change.
            assert_eq!(track_event.scraped_title, None);
            assert_eq!(track_event.genres, vec!["Rock"]);
        }
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }
}
