//! Scenario category 2: library scan / rescan round-trip against a real,
//! isolated `ServerCore` (real SQLite `library.sqlite`, real filesystem
//! walk) — the first category that needs a fully started core, so these
//! tests go through `test_app_with_media_root`.

use super::harness::test_app_with_media_root;
use crate::{get_asset_detail, list_entries, rescan};
use tauri::Manager;

#[tokio::test]
async fn rescan_picks_up_a_new_real_file_and_list_entries_reports_it() {
    let (test_app, root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();

    std::fs::write(
        root_dir.path().join("Test Movie (2020).mp4"),
        b"fake video bytes",
    )
    .expect("write fixture movie file");

    let report = rescan(app.clone(), app.state())
        .await
        .expect("rescan should succeed against a real, isolated media root");
    assert_eq!(report.added, 1, "the new file should be reported as added");

    let entries = list_entries(app.clone(), app.state())
        .await
        .expect("list_entries should succeed");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind, "movie");
    assert_eq!(entries[0].title, "Test Movie");
    assert_eq!(entries[0].year, Some(2020));
}

#[tokio::test]
async fn rescan_is_idempotent_when_nothing_changed_on_disk() {
    let (test_app, root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();

    std::fs::write(
        root_dir.path().join("Test Movie (2020).mp4"),
        b"fake video bytes",
    )
    .expect("write fixture movie file");
    rescan(app.clone(), app.state())
        .await
        .expect("first rescan should succeed");

    let second = rescan(app.clone(), app.state())
        .await
        .expect("second rescan should succeed");
    assert_eq!(second.added, 0);
    assert_eq!(second.updated, 0);
    assert_eq!(second.removed, 0);
    assert_eq!(second.unchanged, 1);
}

#[tokio::test]
async fn rescan_reports_removal_after_a_file_is_deleted() {
    let (test_app, root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();
    let deleted_path = root_dir.path().join("Test Movie (2020).mp4");
    // A second file survives the deletion so the root never looks fully
    // empty — a real, deliberate safety guard (see the "found 0 files ...
    // refusing to treat this as everything was deleted" scan error) treats
    // a root that scans to zero files while the library has known entries
    // as a probably-dropped mount, not a real mass deletion, and refuses to
    // touch the catalog.
    let kept_path = root_dir.path().join("Kept Movie (2021).mp4");

    std::fs::write(&deleted_path, b"fake video bytes").expect("write fixture movie file");
    std::fs::write(&kept_path, b"fake video bytes").expect("write fixture movie file");
    rescan(app.clone(), app.state())
        .await
        .expect("first rescan should succeed");

    std::fs::remove_file(&deleted_path).expect("delete fixture movie file");
    let report = rescan(app.clone(), app.state())
        .await
        .expect("rescan after deletion should succeed");
    assert_eq!(report.removed, 1);
    assert_eq!(report.unchanged, 1);

    let entries = list_entries(app.clone(), app.state())
        .await
        .expect("list_entries should succeed");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title, "Kept Movie");
}

/// The safety guard itself: deleting every file in the only configured root
/// must not silently wipe the library — it's indistinguishable from a
/// dropped network mount, so the real behavior is to refuse and report a
/// real error instead.
#[tokio::test]
async fn rescan_refuses_to_treat_a_fully_empty_root_as_mass_deletion() {
    let (test_app, root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();
    let file_path = root_dir.path().join("Test Movie (2020).mp4");

    std::fs::write(&file_path, b"fake video bytes").expect("write fixture movie file");
    rescan(app.clone(), app.state())
        .await
        .expect("first rescan should succeed");

    std::fs::remove_file(&file_path).expect("delete the only fixture movie file");
    let err = rescan(app.clone(), app.state())
        .await
        .expect_err("scanning to zero files while entries already exist must be refused");
    assert!(
        err.contains("refusing to treat this as"),
        "expected the real empty-root safety-guard message, got: {err}"
    );

    let entries = list_entries(app.clone(), app.state())
        .await
        .expect("list_entries should succeed");
    assert_eq!(
        entries.len(),
        1,
        "the refused scan must leave the existing catalog entry untouched"
    );
}

/// #143: a `.srt` gathered into a movie folder's `Subs/` subfolder is
/// discovered by the rescan, matched to the movie, and surfaced on the
/// asset detail the Media page shows — the same path the device UI uses to
/// offer the track for toggling.
#[tokio::test]
async fn rescan_picks_up_a_subtitle_sidecar_from_a_subs_folder() {
    let (test_app, root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();

    let movie_dir = root_dir.path().join("The Terminator (1984)");
    std::fs::create_dir_all(movie_dir.join("Subs")).expect("create Subs folder");
    std::fs::write(
        movie_dir.join("The Terminator (1984) 1080p.mp4"),
        b"fake video bytes",
    )
    .expect("write fixture movie file");
    std::fs::write(
        movie_dir.join("Subs").join("3_English.srt"),
        b"1\n00:00:01,000 --> 00:00:02,000\nCome with me if you want to live\n",
    )
    .expect("write subtitle sidecar");

    rescan(app.clone(), app.state())
        .await
        .expect("rescan should succeed");

    let entries = list_entries(app.clone(), app.state())
        .await
        .expect("list_entries should succeed");
    assert_eq!(entries.len(), 1, "only the video becomes a catalog entry");
    let entry_key = entries[0].entry_key.clone();

    let detail = get_asset_detail(app.clone(), app.state(), entry_key.clone())
        .await
        .expect("get_asset_detail should succeed");
    assert!(
        detail.subtitle_languages.contains(&"en".to_string()),
        "the Subs-folder sidecar is offered as an English subtitle track: {:?}",
        detail.subtitle_languages
    );

    // Deleting the sidecar clears the track on the next rescan.
    std::fs::remove_file(movie_dir.join("Subs").join("3_English.srt")).expect("remove sidecar");
    rescan(app.clone(), app.state())
        .await
        .expect("second rescan should succeed");
    let detail = get_asset_detail(app.clone(), app.state(), entry_key)
        .await
        .expect("get_asset_detail should succeed");
    assert!(
        detail.subtitle_languages.is_empty(),
        "a removed sidecar is reconciled away: {:?}",
        detail.subtitle_languages
    );
}
