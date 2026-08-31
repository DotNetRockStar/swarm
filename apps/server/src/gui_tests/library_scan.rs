//! Scenario category 2: library scan / rescan round-trip against a real,
//! isolated `ServerCore` (real SQLite `library.sqlite`, real filesystem
//! walk) — the first category that needs a fully started core, so these
//! tests go through `test_app_with_media_root`.

use super::harness::test_app_with_media_root;
use crate::{list_entries, rescan};
use tauri::Manager;

#[tokio::test]
async fn rescan_picks_up_a_new_real_file_and_list_entries_reports_it() {
    let (test_app, root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();

    std::fs::write(root_dir.path().join("Test Movie (2020).mp4"), b"fake video bytes")
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
async fn rescan_reports_scrapeable_titles_for_scene_release_movie_names() {
    let (test_app, root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();
    let cases = [
        (
            "Social Network.2010.BD.Rip.1080p.h264.Rus.Eng.mkv",
            "Social Network",
            2010,
        ),
        (
            "The.Prestige.2006.CUSTOM.MULTi.VF2.1080p.HDLight.AC3.5.1.H264-LiHDL.mkv",
            "The Prestige",
            2006,
        ),
        (
            "Top.Gun.Maverick.2022.IMAX.1080p.Bluray.Atmos.TrueHD.7.1.x264-EVO.mkv",
            "Top Gun Maverick",
            2022,
        ),
        (
            "Waterworld.1995.The.Ulysses.Cut.1080p.BluRay.HEVC.x265-RiPRG.mkv",
            "Waterworld",
            1995,
        ),
    ];
    for (filename, _, _) in cases {
        std::fs::write(root_dir.path().join(filename), b"fake video bytes")
            .expect("write scene-release movie fixture");
    }

    let report = rescan(app.clone(), app.state())
        .await
        .expect("rescan should index scene-release movie names");
    assert_eq!(report.added, cases.len() as u64);

    let entries = list_entries(app.clone(), app.state())
        .await
        .expect("list_entries should return the indexed movies");
    for (_, expected_title, expected_year) in cases {
        let entry = entries
            .iter()
            .find(|entry| entry.title == expected_title)
            .unwrap_or_else(|| panic!("missing UAT entry for {expected_title}"));
        assert_eq!(entry.kind, "movie", "{expected_title}");
        assert_eq!(entry.year, Some(expected_year), "{expected_title}");
    }
}

#[tokio::test]
async fn rescan_is_idempotent_when_nothing_changed_on_disk() {
    let (test_app, root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();

    std::fs::write(root_dir.path().join("Test Movie (2020).mp4"), b"fake video bytes")
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
