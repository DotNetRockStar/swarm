//! Scan/store delta correctness — the Phase 2 exit criteria for the library
//! engine: add, modify, rename, delete are all reflected in entries, the
//! pending-changes queue, the deleted-archive, and the thumbprint.

use std::path::{Path, PathBuf};
use swarm_core::entry_key::entry_key;
use swarm_core::peer::MediaKind;
use swarm_media::roots::MediaRoot;
use swarm_media::scan::{scan_root, scan_roots};
use swarm_media::store::Library;

struct Fixture {
    root: PathBuf,
    library: Library,
    _db_path: PathBuf,
}

async fn fixture(tag: &str) -> Fixture {
    let base = std::env::temp_dir().join(format!("swarm-lib-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("media");
    std::fs::create_dir_all(&root).unwrap();
    let db_path = base.join("library.sqlite");
    let library = Library::open(db_path.to_str().unwrap()).await.unwrap();
    Fixture { root, library, _db_path: db_path }
}

fn write(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[tokio::test]
async fn scan_add_modify_rename_delete() {
    let fx = fixture("delta").await;

    // --- initial add ---
    write(&fx.root, "movies/Heat (1995)/Heat.1995.mkv", &vec![1u8; 4096]);
    write(&fx.root, "music/Artist/Album/01 - Song.flac", b"fake flac bytes");
    write(&fx.root, "movies/Heat (1995)/poster.jpg", b"not media"); // must be ignored
    let report = scan_root(&fx.library, &fx.root).await.unwrap();
    assert_eq!(report.added, 2);
    assert_eq!(report.removed, 0);

    let entries = fx.library.list().await.unwrap();
    assert_eq!(entries.len(), 2);
    let movie = entries.iter().find(|e| e.kind == MediaKind::Movie).unwrap();
    let track = entries.iter().find(|e| e.kind == MediaKind::Track).unwrap();
    // "1995" is a standalone dot-delimited token, captured into `year` and
    // stripped from the title the same way a bracketed year already is —
    // see classify.rs's extract_bare_year_token.
    assert_eq!(movie.title, "Heat");
    assert_eq!(movie.year, Some(1995));
    assert_eq!(track.artist.as_deref(), Some("Artist"));
    assert_eq!(track.track_number, Some(1));
    let thumbprint_v1 = fx.library.thumbprint().await.unwrap();

    // Pending changes queue saw both adds; clearing marks the clean point.
    let changes = fx.library.pending_changes().await.unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|c| c.operation == "upsert"));
    fx.library.clear_pending_changes().await.unwrap();

    // --- unchanged rescan is a no-op ---
    let report = scan_root(&fx.library, &fx.root).await.unwrap();
    assert_eq!(report.unchanged, 2);
    assert_eq!(report.added + report.updated + report.removed, 0);
    assert!(fx.library.pending_changes().await.unwrap().is_empty());
    assert_eq!(fx.library.thumbprint().await.unwrap(), thumbprint_v1);

    // --- modify (size change forces re-fingerprint) ---
    write(&fx.root, "movies/Heat (1995)/Heat.1995.mkv", &vec![2u8; 8192]);
    let report = scan_root(&fx.library, &fx.root).await.unwrap();
    assert_eq!(report.updated, 1);
    let updated = fx.library.get(&movie.entry_key).await.unwrap().unwrap();
    assert_eq!(updated.size, 8192);
    assert_ne!(updated.fingerprint, movie.fingerprint);
    let thumbprint_v2 = fx.library.thumbprint().await.unwrap();
    assert_ne!(thumbprint_v2, thumbprint_v1);
    let changes = fx.library.pending_changes().await.unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].entry_key, movie.entry_key);
    fx.library.clear_pending_changes().await.unwrap();

    // --- rename = delete old path + add new path ---
    std::fs::rename(
        fx.root.join("movies/Heat (1995)/Heat.1995.mkv"),
        fx.root.join("movies/Heat (1995)/Heat.Remastered.mkv"),
    )
    .unwrap();
    let report = scan_root(&fx.library, &fx.root).await.unwrap();
    assert_eq!((report.added, report.removed), (1, 1));
    assert!(fx.library.get(&movie.entry_key).await.unwrap().is_none());
    let entries = fx.library.list().await.unwrap();
    let renamed = entries.iter().find(|e| e.kind == MediaKind::Movie).unwrap();
    assert_eq!(renamed.relative_path, "movies/Heat (1995)/Heat.Remastered.mkv");
    // Same bytes, new path: content identity survives the rename.
    assert_eq!(renamed.fingerprint, updated.fingerprint);
    let changes = fx.library.pending_changes().await.unwrap();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().any(|c| c.operation == "delete" && c.entry_key == movie.entry_key));
    assert!(changes.iter().any(|c| c.operation == "upsert" && c.entry_key == renamed.entry_key));
    fx.library.clear_pending_changes().await.unwrap();

    // --- delete ---
    std::fs::remove_file(fx.root.join("music/Artist/Album/01 - Song.flac")).unwrap();
    let report = scan_root(&fx.library, &fx.root).await.unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(fx.library.list().await.unwrap().len(), 1);
    let changes = fx.library.pending_changes().await.unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].operation, "delete");
    assert_eq!(changes[0].entry_key, track.entry_key);
}

#[tokio::test]
async fn single_root_scan_roots_matches_scan_root_byte_for_byte() {
    // Backward-compat guarantee: scan_root (single PathBuf) must still
    // produce the exact relative_path/entry_key values it always has, since
    // it's a thin wrapper over scan_roots with one implicit "local" root
    // that scan_roots never prefixes when there's only one root configured.
    let fx = fixture("single-root-parity").await;
    write(&fx.root, "movies/Heat (1995)/Heat.1995.mkv", &vec![1u8; 4096]);
    scan_root(&fx.library, &fx.root).await.unwrap();
    let entries = fx.library.list().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].relative_path, "movies/Heat (1995)/Heat.1995.mkv");
    assert_eq!(entries[0].entry_key, entry_key("movies/Heat (1995)/Heat.1995.mkv"));
}

#[tokio::test]
async fn two_roots_with_the_same_relative_path_get_distinct_entry_keys() {
    let base = std::env::temp_dir().join(format!("swarm-lib-two-roots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root_a = base.join("root-a");
    let root_b = base.join("root-b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let library = Library::open(base.join("library.sqlite").to_str().unwrap()).await.unwrap();

    // Same sub-path under both roots — must not collide.
    write(&root_a, "movies/Heat (1995)/Heat.1995.mkv", &vec![1u8; 4096]);
    write(&root_b, "movies/Heat (1995)/Heat.1995.mkv", &vec![2u8; 4096]);

    let roots = [
        MediaRoot { label: "local".to_string(), path: root_a.clone() },
        MediaRoot { label: "nas".to_string(), path: root_b.clone() },
    ];
    let report = scan_roots(&library, &roots).await.unwrap();
    assert_eq!(report.added, 2);

    let entries = library.list().await.unwrap();
    assert_eq!(entries.len(), 2);
    let paths: Vec<&str> = entries.iter().map(|e| e.relative_path.as_str()).collect();
    assert!(paths.contains(&"local/movies/Heat (1995)/Heat.1995.mkv"));
    assert!(paths.contains(&"nas/movies/Heat (1995)/Heat.1995.mkv"));
    assert_ne!(entries[0].entry_key, entries[1].entry_key);
    assert_ne!(entries[0].fingerprint, entries[1].fingerprint); // distinct content too
}

#[tokio::test]
async fn manual_metadata_overwrites_display_fields_and_leaves_grouping_fields_untouched() {
    let fx = fixture("manual-metadata").await;
    write(&fx.root, "music/Artist/Album/01 - Song.flac", b"fake flac bytes");
    scan_root(&fx.library, &fx.root).await.unwrap();
    let entry = fx.library.list().await.unwrap().into_iter().next().unwrap();
    assert_eq!(entry.artist.as_deref(), Some("Artist"));
    assert_eq!(entry.album.as_deref(), Some("Album"));

    // Only title provided — genres must stay untouched (None means "leave
    // unchanged", not "clear").
    fx.library.set_manual_metadata(&entry.entry_key, Some("Manual Title"), None).await.unwrap();
    let after_title = fx.library.get(&entry.entry_key).await.unwrap().unwrap();
    assert_eq!(after_title.scraped_title.as_deref(), Some("Manual Title"));
    assert!(after_title.genres.is_empty());
    // Grouping fields are never touched by a manual edit.
    assert_eq!(after_title.artist.as_deref(), Some("Artist"));
    assert_eq!(after_title.album.as_deref(), Some("Album"));

    // A title having been set marks the entry as "processed" for the bulk
    // scraper's purposes, same as a real scrape result would.
    assert!(fx.library.missing_scrape().await.unwrap().is_empty());

    // Now set genres too, and clear the title explicitly via Some("").
    fx.library
        .set_manual_metadata(&entry.entry_key, Some(""), Some(&["Electronic".to_string()]))
        .await
        .unwrap();
    let after_genres = fx.library.get(&entry.entry_key).await.unwrap().unwrap();
    assert_eq!(after_genres.scraped_title, None); // empty string is the "no title override" sentinel
    assert_eq!(after_genres.genres, vec!["Electronic"]);
    assert_eq!(after_genres.artist.as_deref(), Some("Artist"));
}

#[tokio::test]
async fn clear_scrape_result_reverts_to_unscraped_and_leaves_grouping_fields_untouched() {
    use swarm_media::store::ArtworkKind;

    let fx = fixture("clear-scrape").await;
    write(&fx.root, "movies/Heat (1995)/Heat.1995.mkv", &[1u8; 10]);
    scan_root(&fx.library, &fx.root).await.unwrap();
    let entry = fx.library.list().await.unwrap().into_iter().next().unwrap();

    fx.library
        .set_scrape_result(&entry.entry_key, Some("Heat"), &["Crime".to_string()], &[])
        .await
        .unwrap();
    fx.library.set_artwork(&entry.entry_key, ArtworkKind::Poster, "movies/Heat (1995)/images/poster.jpg").await.unwrap();
    let scraped = fx.library.get(&entry.entry_key).await.unwrap().unwrap();
    assert_eq!(scraped.scraped_title.as_deref(), Some("Heat"));
    assert!(fx.library.missing_scrape().await.unwrap().is_empty());

    let cleared_paths = fx.library.clear_scrape_result(&entry.entry_key).await.unwrap();
    assert_eq!(cleared_paths, vec!["movies/Heat (1995)/images/poster.jpg".to_string()]);

    let reverted = fx.library.get(&entry.entry_key).await.unwrap().unwrap();
    assert_eq!(reverted.scraped_title, None);
    assert!(reverted.genres.is_empty());
    assert!(fx.library.artwork(&entry.entry_key, ArtworkKind::Poster).await.unwrap().is_none());
    // Path-derived fields are never touched by a scrape (or its reversal).
    assert_eq!(reverted.relative_path, scraped.relative_path);
    // Reverting must put it back into the bulk-scraper's "needs work" set.
    let missing = fx.library.missing_scrape().await.unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].entry_key, entry.entry_key);
}

#[tokio::test]
async fn thumbprint_is_order_independent_and_content_sensitive() {
    let fx_a = fixture("tp-a").await;
    let fx_b = fixture("tp-b").await;
    // Same content written in different order → same thumbprint.
    write(&fx_a.root, "movies/A.mkv", b"aaaa");
    write(&fx_a.root, "movies/B.mkv", b"bbbb");
    write(&fx_b.root, "movies/B.mkv", b"bbbb");
    write(&fx_b.root, "movies/A.mkv", b"aaaa");
    scan_root(&fx_a.library, &fx_a.root).await.unwrap();
    scan_root(&fx_b.library, &fx_b.root).await.unwrap();
    assert_eq!(fx_a.library.thumbprint().await.unwrap(), fx_b.library.thumbprint().await.unwrap());
}

#[tokio::test]
async fn reclassify_all_repairs_stale_bonus_content_and_leaves_correct_entries_untouched() {
    // scan_root always runs the CURRENT classify(), so it can't be used to
    // manufacture a stale pre-fix state — write the DB row directly instead,
    // exactly as the old buggy classify() would have (filed as a Movie, with
    // real-looking-but-wrong scraped data), matching the real bug this
    // guards: bonus content under a show's season folder scraped as if it
    // were a completely unrelated standalone movie.
    use swarm_media::classify::classify;
    use swarm_media::store::{ArtworkKind, EntryRecord};

    let fx = fixture("reclassify").await;

    let wrong_relative = "Shows/Dexter/Dexter (2006) S03/Featurettes/Interviews/Michael C. Hall.mkv";
    let wrong_entry = EntryRecord {
        entry_key: entry_key(wrong_relative),
        relative_path: wrong_relative.to_string(),
        kind: MediaKind::Movie,
        title: "Michael C Hall".to_string(),
        size: 10,
        modified_time: 0,
        fingerprint: "fp1".to_string(),
        artist: None,
        album: None,
        track_number: None,
        show_title: None,
        season: None,
        episode: None,
        year: None,
        duration_secs: None,
        video: None,
        audio: None,
        scraped_title: None,
        genres: vec![],
        artwork_version: 0,
        cast: vec![],
    };
    fx.library.upsert(&wrong_entry).await.unwrap();
    // upsert() deliberately never writes scrape/artwork columns (a rescan
    // must never clobber existing scrape results) — set the "already
    // (wrongly) scraped" state the way a real scrape actually would.
    fx.library.set_scrape_result(&wrong_entry.entry_key, Some("The Interview"), &["Comedy".to_string()], &[]).await.unwrap();
    fx.library
        .set_artwork(&wrong_entry.entry_key, ArtworkKind::Poster, "Shows/Dexter/Dexter (2006) S03/images/wrong-poster.jpg")
        .await
        .unwrap();

    // A genuinely correct entry — a real numbered episode — that reclassify
    // must leave completely untouched, including its existing scrape data.
    let correct_relative = "Shows/Dexter/Dexter (2006) S03/Dexter.S03E01.Our Father.mkv";
    let correct_classified = classify(correct_relative).unwrap();
    let correct_entry = EntryRecord {
        entry_key: entry_key(correct_relative),
        relative_path: correct_relative.to_string(),
        kind: correct_classified.kind,
        title: correct_classified.title.clone(),
        size: 20,
        modified_time: 0,
        fingerprint: "fp2".to_string(),
        artist: None,
        album: None,
        track_number: None,
        show_title: correct_classified.show_title.clone(),
        season: correct_classified.season,
        episode: correct_classified.episode,
        year: correct_classified.year,
        duration_secs: None,
        video: None,
        audio: None,
        scraped_title: None,
        genres: vec![],
        artwork_version: 0,
        cast: vec![],
    };
    fx.library.upsert(&correct_entry).await.unwrap();
    fx.library.set_scrape_result(&correct_entry.entry_key, Some("Dexter"), &["Drama".to_string()], &[]).await.unwrap();

    let report = fx.library.reclassify_all().await.unwrap();
    assert_eq!(report.changed, 1);
    assert_eq!(report.unchanged, 1);

    let fixed = fx.library.get(&wrong_entry.entry_key).await.unwrap().unwrap();
    assert_eq!(fixed.kind, MediaKind::Episode);
    assert_eq!(fixed.show_title.as_deref(), Some("Dexter"));
    assert_eq!(fixed.season, Some(0));
    assert_eq!(fixed.episode, None);
    assert_eq!(fixed.scraped_title, None, "stale wrong scrape data must be cleared, not just relabeled");
    assert!(fixed.genres.is_empty());
    assert!(
        fx.library.artwork(&fixed.entry_key, ArtworkKind::Poster).await.unwrap().is_none(),
        "the wrong movie's artwork must be cleared too"
    );

    let untouched = fx.library.get(&correct_entry.entry_key).await.unwrap().unwrap();
    assert_eq!(untouched.scraped_title.as_deref(), Some("Dexter"), "already-correct entries must be left completely untouched");
    assert_eq!(untouched.genres, vec!["Drama".to_string()]);
}
