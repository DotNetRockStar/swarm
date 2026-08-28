//! Scenario category 5: manual metadata/artwork editing against a real
//! scanned entry.

use super::harness::test_app_with_media_root;
use crate::{list_entries, rescan, set_manual_metadata};
use tauri::Manager;

async fn seed_one_entry(app: &tauri::AppHandle<tauri::test::MockRuntime>, root_dir: &std::path::Path) -> String {
    std::fs::write(root_dir.join("Test Movie (2020).mp4"), b"fake video bytes")
        .expect("write fixture movie file");
    rescan(app.clone(), app.state())
        .await
        .expect("rescan should succeed");
    let entries = list_entries(app.clone(), app.state())
        .await
        .expect("list_entries should succeed");
    entries[0].entry_key.clone()
}

#[tokio::test]
async fn set_manual_metadata_overrides_title_genres_overview_and_rating() {
    let (test_app, root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();
    let entry_key = seed_one_entry(&app, root_dir.path()).await;

    set_manual_metadata(
        app.clone(),
        app.state(),
        entry_key.clone(),
        Some("Renamed Movie".to_string()),
        Some(vec!["Action".to_string(), "Sci-Fi".to_string()]),
        Some("A manually-entered overview.".to_string()),
        Some("PG-13".to_string()),
    )
    .await
    .expect("set_manual_metadata should succeed for a real entry");

    let entries = list_entries(app.clone(), app.state())
        .await
        .expect("list_entries should succeed");
    let entry = entries
        .iter()
        .find(|e| e.entry_key == entry_key)
        .expect("the edited entry should still be present");
    // `title` stays the path-derived name (see `classify.rs`'s grouping-key
    // invariant); the manual override lands in the separate `scraped_title`
    // display-overlay field instead.
    assert_eq!(entry.scraped_title.as_deref(), Some("Renamed Movie"));
    assert_eq!(entry.genres, vec!["Action".to_string(), "Sci-Fi".to_string()]);
    assert_eq!(entry.overview.as_deref(), Some("A manually-entered overview."));
    assert_eq!(entry.rating.as_deref(), Some("PG-13"));
}

/// `Library::set_manual_metadata` is a plain `UPDATE ... WHERE entry_key =
/// ?` with no existence check, so an unknown key affects zero rows and
/// still returns `Ok(())` rather than an error — documenting the real
/// behavior here (not asserting the error path this command doesn't have).
#[tokio::test]
async fn set_manual_metadata_on_an_unknown_entry_key_is_a_silent_no_op() {
    let (test_app, _root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();

    set_manual_metadata(
        app.clone(),
        app.state(),
        "does-not-exist".to_string(),
        Some("Whatever".to_string()),
        None,
        None,
        None,
    )
    .await
    .expect("an unknown entry_key does not error");

    let entries = list_entries(app.clone(), app.state())
        .await
        .expect("list_entries should succeed");
    assert!(entries.is_empty(), "no entry should have been created");
}
