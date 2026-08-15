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

use crate::scrape::artwork;
use crate::scrape::coverart::{CoverArtClient, CoverArtError};
use crate::scrape::musicbrainz::{MbError, MusicBrainzClient};
use crate::scrape::tmdb::{ScrapedVideo, TmdbClient, TmdbError};
use crate::scrape::wikimedia::WikimediaClient;
use crate::store::{ArtworkKind, EntryRecord, Library};
use std::collections::HashMap;
use std::path::Path;
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
    media_root: &Path,
    config: &ScrapeConfig,
    cancel: &AtomicBool,
) -> sqlx::Result<BulkScrapeReport> {
    let mut report = BulkScrapeReport::default();
    let entries = library.missing_scrape().await?;
    let (videos, tracks): (Vec<EntryRecord>, Vec<EntryRecord>) =
        entries.into_iter().partition(|e| matches!(e.kind, MediaKind::Movie | MediaKind::Episode));

    scrape_videos(library, media_root, config, &videos, cancel, &mut report).await?;
    if !cancel.load(Ordering::Relaxed) {
        scrape_tracks(library, media_root, config, &tracks, cancel, &mut report).await?;
    }
    Ok(report)
}

async fn scrape_videos(
    library: &Library,
    media_root: &Path,
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
            MediaKind::Movie => tmdb.search_and_fetch_movie(&entry.title).await,
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
                library.set_scrape_result(&entry.entry_key, Some(&scraped.title), &scraped.genres).await?;
                if let Some(url) = &scraped.poster_url {
                    save_video_artwork(library, media_root, entry, ArtworkKind::Poster, "poster", url).await;
                }
                if let Some(url) = &scraped.backdrop_url {
                    save_video_artwork(library, media_root, entry, ArtworkKind::Backdrop, "backdrop", url).await;
                }
                report.matched += 1;
            }
            Err(TmdbError::NotFound) => {
                library.set_scrape_result(&entry.entry_key, None, &[]).await?;
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
    media_root: &Path,
    entry: &EntryRecord,
    kind: ArtworkKind,
    label: &str,
    url: &str,
) {
    let Ok(bytes) = download_bytes(url).await else { return };
    let filename = format!("{}-tmdb-{label}.jpg", artwork::sanitize_stem(artwork::file_stem(&entry.relative_path)));
    if let Ok(relative) = artwork::save_artwork(media_root, &entry.relative_path, &filename, &bytes).await {
        let _ = library.set_artwork(&entry.entry_key, kind, &relative).await;
    }
}

async fn scrape_tracks(
    library: &Library,
    media_root: &Path,
    config: &ScrapeConfig,
    entries: &[EntryRecord],
    cancel: &AtomicBool,
    report: &mut BulkScrapeReport,
) -> sqlx::Result<()> {
    let mb = config.musicbrainz_base.as_deref().map_or_else(MusicBrainzClient::new, MusicBrainzClient::with_base_url);
    let coverart = config.coverart_base.as_deref().map_or_else(CoverArtClient::new, CoverArtClient::with_base_url);
    let wikimedia = config.wikimedia_base.as_deref().map_or_else(WikimediaClient::new, WikimediaClient::with_base_url);

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
        match mb.search_release(&artist, &album).await {
            Err(MbError::NotFound) => {
                for track in &group {
                    library.set_scrape_result(&track.entry_key, None, &[]).await?;
                }
                report.not_found += group.len() as u64;
            }
            Err(MbError::Unavailable(reason)) => {
                tracing::debug!(%artist, %album, %reason, "musicbrainz unavailable, will retry next run");
                report.failed += group.len() as u64;
            }
            Ok(release_mbid) => {
                // Release-level data only, applied to every track in the
                // group — see the module docs on why this is per-album, not
                // per-track (matches Drone's music bulk-scrape behavior).
                let details = mb.release_lookup(&release_mbid).await.ok();
                let genres = details.as_ref().map(|d| d.genres.clone()).unwrap_or_default();
                for track in &group {
                    library.set_scrape_result(&track.entry_key, None, &genres).await?;
                }
                report.matched += group.len() as u64;

                if let Some(first) = group.first() {
                    if let Ok(cover) = coverart.front_cover(&release_mbid).await {
                        if let Ok(relative) =
                            artwork::save_artwork(media_root, &first.relative_path, "album-cover.jpg", &cover).await
                        {
                            for track in &group {
                                let _ = library.set_artwork(&track.entry_key, ArtworkKind::Cover, &relative).await;
                            }
                        }
                    }
                    if let Some(artist_mbid) = details.as_ref().and_then(|d| d.artist_mbid.as_deref()) {
                        if let Some(bytes) = fetch_artist_photo(&mb, &wikimedia, artist_mbid).await {
                            if let Ok(relative) =
                                artwork::save_artwork(media_root, &first.relative_path, "artist-photo.jpg", &bytes).await
                            {
                                for track in &group {
                                    let _ =
                                        library.set_artwork(&track.entry_key, ArtworkKind::ArtistPhoto, &relative).await;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
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
            run_bulk_scrape(&library, &root, &ScrapeConfig::default(), &AtomicBool::new(false)).await.unwrap();
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
                    Json(json!({"title": "Heat", "genres": [{"name": "Crime"}], "poster_path": "/p.jpg"}))
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
        let report = run_bulk_scrape(&library, &root, &config, &AtomicBool::new(false)).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 1, not_found: 0, failed: 0, skipped: 0 });

        let entry = &library.list().await.unwrap()[0];
        assert_eq!(entry.scraped_title.as_deref(), Some("Heat"));
        assert_eq!(entry.genres, vec!["Crime"]);
        assert_eq!(entry.artwork_version, 1);
        let (art_path, art_version) = library.artwork(&entry.entry_key, ArtworkKind::Poster).await.unwrap().unwrap();
        assert_eq!(art_version, 1);
        assert_eq!(std::fs::read(root.join(&art_path)).unwrap(), vec![9, 9, 9]);
        assert!(library.missing_scrape().await.unwrap().is_empty());
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
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
        let report = run_bulk_scrape(&library, &root, &config, &AtomicBool::new(false)).await.unwrap();
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
        let report = run_bulk_scrape(&library, &root, &config, &AtomicBool::new(false)).await.unwrap();
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
            run_bulk_scrape(&library, &root, &ScrapeConfig::default(), &AtomicBool::new(false)).await.unwrap();
        assert_eq!(report, BulkScrapeReport { matched: 0, not_found: 0, failed: 0, skipped: 1 });
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }
}
