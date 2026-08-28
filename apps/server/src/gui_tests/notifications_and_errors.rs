//! Scenario category 4: notifications and client-errors management. Errors
//! are seeded directly through `Library::record_client_error` — the same
//! write path a real client's error report takes over the wire (see
//! `swarm_core::peer::ClientErrorReport`), just without needing a real TV to
//! send one — so the commands under test still run against real inserted
//! SQLite rows, not stubs.

use super::harness::test_app_with_media_root;
use crate::{clear_client_errors, list_client_errors, resolve_client_error};
use swarm_core::peer::ClientErrorReport;
use tauri::Manager;

fn fixture_report() -> ClientErrorReport {
    ClientErrorReport {
        device_id: "test-device-1".to_string(),
        device_name: "Test Fire TV".to_string(),
        entry_key: None,
        asset_title: None,
        kind: None,
        message: "playback failed: connection reset".to_string(),
        context: None,
        occurred_at_ms: 0,
    }
}

#[tokio::test]
async fn list_client_errors_reports_a_real_seeded_error() {
    let (test_app, _root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();
    let core = app
        .state::<crate::AppState>()
        .core(&app)
        .await
        .expect("core should initialize");
    core.library
        .record_client_error(&fixture_report())
        .await
        .expect("seed a real client error row");

    let errors = list_client_errors(app.clone(), app.state())
        .await
        .expect("list_client_errors should succeed");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].device_name, "Test Fire TV");
    assert_eq!(errors[0].message, "playback failed: connection reset");
    assert!(errors[0].resolved_at_ms.is_none());
}

#[tokio::test]
async fn resolve_client_error_marks_it_resolved_and_rejects_a_second_resolve() {
    let (test_app, _root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();
    let core = app
        .state::<crate::AppState>()
        .core(&app)
        .await
        .expect("core should initialize");
    core.library
        .record_client_error(&fixture_report())
        .await
        .expect("seed a real client error row");
    let errors = list_client_errors(app.clone(), app.state())
        .await
        .expect("list_client_errors should succeed");
    let id = errors[0].id;

    resolve_client_error(app.clone(), app.state(), id, Some("known issue".to_string()))
        .await
        .expect("resolving an existing error should succeed");

    let errors = list_client_errors(app.clone(), app.state())
        .await
        .expect("list_client_errors should succeed");
    assert!(errors[0].resolved_at_ms.is_some());
    assert_eq!(errors[0].resolution_comments.as_deref(), Some("known issue"));

    let err = resolve_client_error(app.clone(), app.state(), id, None)
        .await
        .expect_err("resolving the same error twice must be rejected");
    assert!(err.contains("already resolved"), "got: {err}");
}

#[tokio::test]
async fn clear_client_errors_empties_the_list() {
    let (test_app, _root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();
    let core = app
        .state::<crate::AppState>()
        .core(&app)
        .await
        .expect("core should initialize");
    core.library
        .record_client_error(&fixture_report())
        .await
        .expect("seed a real client error row");

    clear_client_errors(app.clone(), app.state())
        .await
        .expect("clear_client_errors should succeed");

    let errors = list_client_errors(app.clone(), app.state())
        .await
        .expect("list_client_errors should succeed");
    assert!(errors.is_empty());
}
