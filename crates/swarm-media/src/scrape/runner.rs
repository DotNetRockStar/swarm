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

use crate::roots::RootResolver;
use crate::scrape::artwork;
use crate::scrape::coverart::{CoverArtClient, CoverArtError};
use crate::scrape::musicbrainz::{MbError, MusicBrainzClient};
use crate::scrape::tmdb::{ScrapedVideo, TmdbClient, TmdbError, TmdbOverride};
use crate::scrape::wikimedia::WikimediaClient;
use crate::store::{ArtworkKind, EntryRecord, Library};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use swarm_core::peer::MediaKind;

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

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BulkScrapeReport {
    pub matched: u64,
    pub not_found: u64,
    pub failed: u64,
    pub skipped: u64,
}

pub async fn run_bulk_scrape(
    library: &Library,
    roots: &RootResolver,
    config: &ScrapeConfig,
    cancel: &AtomicBool,
) -> sqlx::Result<BulkScrapeReport> {
    let mut report = BulkScrapeReport::default();
    let entries = library.missing_scrape().await?;
    let (videos, tracks): (Vec<EntryRecord>, Vec<EntryRecord>) =
        entries.into_iter().partition(|e| matches!(e.kind, MediaKind::Movie | MediaKind::Episode));

    scrape_videos(library, roots, config, &videos, cancel, &mut report).await?;
    if !cancel.load(Ordering::Relaxed) {
        scrape_tracks(library, roots, config, &tracks, cancel, &mut report).await?;
    }
    Ok(report)
}

async fn scrape_videos(
    library: &Library,
    roots: &RootResolver,
    config: &ScrapeConfig,
    entries: &[EntryRecord],
    cancel: &AtomicBool,
    report: &mut BulkScrapeReport,
) -> sqlx::Result<()> {
    let Some(api_key) = &config.tmdb_api_key else {
        report.skipped += entries.len() as u64;
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
            MediaKind::Movie => tmdb.search_and_fetch_movie(&entry.title, entry.year).await,
            MediaKind::Episode => {
                let query = entry.show_title.clone().unwrap_or_else(|| entry.title.clone());
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
                if let Some(url) = &scraped.poster_url {
                    save_video_artwork(library, roots, entry, ArtworkKind::Poster, "poster", url).await;
                }
                if let Some(url) = &scraped.backdrop_url {
                    save_video_artwork(library, roots, entry, ArtworkKind::Backdrop, "backdrop", url).await;
                }
                report.matched += 1;
            }
            Err(TmdbError::NotFound) => {
                library.set_scrape_result(&entry.entry_key, None, &[], &[]).await?;
                report.not_found += 1;
            }
            Err(TmdbError::Unavailable(reason)) => {
                tracing::debug!(entry = %entry.entry_key, %reason, "tmdb unavailable, will retry next run");
                report.failed += 1;
            }
        }
    }
    Ok(())
}

async fn save_video_artwork(
    library: &Library,
    roots: &RootResolver,
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
    roots: &RootResolver,
    config: &ScrapeConfig,
    entries: &[EntryRecord],
    cancel: &AtomicBool,
    report: &mut BulkScrapeReport,
) -> sqlx::Result<()> {
    let scrapers = MusicScrapers::from_config(config);

    let mut groups: HashMap<(String, String), Vec<&EntryRecord>> = HashMap::new();
    for entry in entries {
        match (&entry.artist, &entry.album) {
            (Some(artist), Some(album)) if !artist.is_empty() && !album.is_empty() => {
                groups.entry((artist.clone(), album.clone())).or_default().push(entry);
            }
            _ => report.skipped += 1,
        }
    }

    for ((artist, album), group) in groups {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        scrape_one_album_group(library, roots, &scrapers, &artist, &album, &group, report).await?;
    }
    Ok(())
}

/// One (artist, album) group's worth of MusicBrainz + Cover Art Archive +
/// Wikimedia work — release-level data applied to every track in the group
/// (see the module docs on why this is per-album, not per-track). Shared by
/// the bulk loop above and [`scrape_one_track`]'s pinpoint path so a single
/// manually-triggered rescrape of one track re-syncs its whole album, same
/// as a bulk run would.
async fn scrape_one_album_group(
    library: &Library,
    roots: &RootResolver,
    scrapers: &MusicScrapers,
    artist: &str,
    album: &str,
    group: &[&EntryRecord],
    report: &mut BulkScrapeReport,
) -> sqlx::Result<()> {
    let MusicScrapers { mb, coverart, wikimedia } = scrapers;
    match mb.search_release(artist, album).await {
        Err(MbError::NotFound) => {
            for track in group {
                library.set_scrape_result(&track.entry_key, None, &[], &[]).await?;
            }
            report.not_found += group.len() as u64;
        }
        Err(MbError::Unavailable(reason)) => {
            tracing::debug!(%artist, %album, %reason, "musicbrainz unavailable, will retry next run");
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
    roots: &RootResolver,
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
            MediaKind::Movie => tmdb.search_and_fetch_movie(&entry.title, entry.year).await?,
            MediaKind::Episode => {
                let query = entry.show_title.clone().unwrap_or_else(|| entry.title.clone());
                tmdb.search_and_fetch_tv(&query).await?
            }
            MediaKind::Track => unreachable!("checked above"),
        },
    };
    library.set_scrape_result(&entry.entry_key, Some(&scraped.title), &scraped.genres, &scraped.cast).await?;
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
    roots: &RootResolver,
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
    scrape_one_album_group(library, roots, &scrapers, artist, album, &group, &mut report).await?;
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

    fn resolver(root: &std::path::Path) -> RootResolver {
        RootResolver::single(root.to_path_buf())
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
            run_bulk_scrape(&library, &resolver(&root), &ScrapeConfig::default(), &AtomicBool::new(false)).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 0, not_found: 0, failed: 0, skipped: 1 });
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
                        "credits": {"cast": [{"name": "Al Pacino", "character": "Vincent Hanna"}]}
                    }))
                }),
            )
            .route("/img/w500/p.jpg", get(|| async { [9u8, 9, 9] }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let config = ScrapeConfig {
            tmdb_api_key: Some("key".into()),
            tmdb_api_base: Some(format!("http://{addr}")),
            tmdb_image_base: Some(format!("http://{addr}/img")),
            ..Default::default()
        };
        let report = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false)).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 1, not_found: 0, failed: 0, skipped: 0 });

        let entry = &library.list().await.unwrap()[0];
        assert_eq!(entry.scraped_title.as_deref(), Some("Heat"));
        assert_eq!(entry.genres, vec!["Crime"]);
        assert_eq!(entry.cast.len(), 1);
        assert_eq!(entry.cast[0].name, "Al Pacino");
        assert_eq!(entry.cast[0].character.as_deref(), Some("Vincent Hanna"));
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
        run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false)).await.unwrap();
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
        assert_eq!(report, BulkScrapeReport { matched: 2, not_found: 0, failed: 0, skipped: 0 });
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
        let report = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false)).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 0, not_found: 1, failed: 0, skipped: 0 });
        // Processed (no match) still counts as done — must not be re-queued.
        assert!(library.missing_scrape().await.unwrap().is_empty());
        let entry = &library.list().await.unwrap()[0];
        assert_eq!(entry.scraped_title, None);
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
        let report = run_bulk_scrape(&library, &resolver(&root), &config, &AtomicBool::new(false)).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 2, not_found: 0, failed: 0, skipped: 0 });

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
            run_bulk_scrape(&library, &resolver(&root), &ScrapeConfig::default(), &AtomicBool::new(false)).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 0, not_found: 0, failed: 0, skipped: 1 });
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }
}
