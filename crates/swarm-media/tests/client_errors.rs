//! `/errors/report` and the `client_errors` store — the server side of a
//! client calling back with a playback/catalog failure for triage on the
//! swarm page.

use std::sync::Arc;
use swarm_core::peer::{ClientErrorReport, PeerRequest};
use swarm_media::serve::MediaService;
use swarm_media::store::Library;

fn report_request(report: Option<ClientErrorReport>) -> PeerRequest {
    PeerRequest {
        path: "/errors/report".into(),
        range: None,
        if_none_match: None,
        playback: None,
        error_report: report,
        like: None,
    }
}

fn sample_report() -> ClientErrorReport {
    ClientErrorReport {
        device_id: "device-1".into(),
        device_name: "Living Room Fire TV".into(),
        entry_key: Some("0123456789abcdef01234567".into()),
        asset_title: Some("Outcry".into()),
        kind: Some("episode".into()),
        message: "server could not prepare playback (503): moov atom not found".into(),
        context: Some("HTTP 503".into()),
        occurred_at_ms: 1_700_000_000_000,
    }
}

async fn service_with_fresh_library() -> (MediaService, Arc<Library>) {
    let root = std::env::temp_dir().join(format!(
        "swarm-client-errors-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let library = Arc::new(
        Library::open(root.join("library.sqlite").to_str().unwrap())
            .await
            .unwrap(),
    );
    let service = MediaService::new(library.clone(), root.join("media"));
    (service, library)
}

// No external `rand` dependency in this crate's dev-deps. A bare nanosecond
// timestamp isn't actually unique across threads that start within the same
// clock tick — confirmed live: two of this file's `#[tokio::test]`s (which
// the default harness runs concurrently on separate OS threads) landed on
// the same temp dir and fought over the same SQLite file. The atomic counter
// guarantees uniqueness within this process regardless of clock resolution.
fn rand_suffix() -> u128 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    now + n as u128
}

#[tokio::test]
async fn reported_error_is_persisted_and_listed_newest_first() {
    let (service, library) = service_with_fresh_library().await;

    let first = service
        .resolve(&report_request(Some(sample_report())))
        .await;
    assert_eq!(first.header.status, 204);

    let mut second_report = sample_report();
    second_report.message = "catalog unreachable".into();
    second_report.entry_key = None;
    second_report.asset_title = None;
    second_report.occurred_at_ms += 1000;
    let second = service.resolve(&report_request(Some(second_report))).await;
    assert_eq!(second.header.status, 204);

    let errors = library.list_client_errors().await.unwrap();
    assert_eq!(errors.len(), 2);
    // Newest (second, received later) first.
    assert_eq!(errors[0].message, "catalog unreachable");
    assert_eq!(errors[0].entry_key, None);
    assert_eq!(errors[1].message, sample_report().message);
    assert_eq!(errors[1].device_name, "Living Room Fire TV");
    assert_eq!(errors[1].asset_title.as_deref(), Some("Outcry"));

    assert_eq!(library.client_error_count().await.unwrap(), 2);
}

#[tokio::test]
async fn request_with_no_error_report_body_is_rejected() {
    let (service, _library) = service_with_fresh_library().await;
    let resolved = service.resolve(&report_request(None)).await;
    assert_eq!(resolved.header.status, 400);
}

#[tokio::test]
async fn request_with_an_empty_device_id_or_message_is_rejected() {
    let (service, library) = service_with_fresh_library().await;

    let mut missing_device = sample_report();
    missing_device.device_id = String::new();
    assert_eq!(
        service
            .resolve(&report_request(Some(missing_device)))
            .await
            .header
            .status,
        400
    );

    let mut missing_message = sample_report();
    missing_message.message = String::new();
    assert_eq!(
        service
            .resolve(&report_request(Some(missing_message)))
            .await
            .header
            .status,
        400
    );

    assert_eq!(library.client_error_count().await.unwrap(), 0);
}

#[tokio::test]
async fn delete_and_clear_remove_from_the_store() {
    let (service, library) = service_with_fresh_library().await;
    service
        .resolve(&report_request(Some(sample_report())))
        .await;
    let mut other = sample_report();
    other.message = "a second error".into();
    service.resolve(&report_request(Some(other))).await;

    let errors = library.list_client_errors().await.unwrap();
    assert_eq!(errors.len(), 2);
    library.delete_client_error(errors[0].id).await.unwrap();
    assert_eq!(library.client_error_count().await.unwrap(), 1);

    library.clear_client_errors().await.unwrap();
    assert_eq!(library.client_error_count().await.unwrap(), 0);
}

#[tokio::test]
async fn resolving_notifies_only_the_reporting_device_until_dismissed() {
    let (service, library) = service_with_fresh_library().await;
    service
        .resolve(&report_request(Some(sample_report())))
        .await;
    let error = library.list_client_errors().await.unwrap().remove(0);

    assert!(library
        .resolve_client_error(error.id, Some("Replaced the damaged media file."))
        .await
        .unwrap());
    assert!(!library
        .resolve_client_error(error.id, Some("A second resolution must not re-notify."))
        .await
        .unwrap());
    assert_eq!(library.client_error_count().await.unwrap(), 0);

    let other_device = service
        .resolve(&PeerRequest {
            path: "/notifications/device-2".into(),
            range: None,
            if_none_match: None,
            playback: None,
            error_report: None,
            like: None,
        })
        .await;
    assert_eq!(other_device.header.status, 200);
    assert_eq!(other_device.header.len, 2); // []

    let listed = service
        .resolve(&PeerRequest {
            path: "/notifications/device-1".into(),
            range: None,
            if_none_match: None,
            playback: None,
            error_report: None,
            like: None,
        })
        .await;
    assert_eq!(listed.header.status, 200);
    let swarm_media::serve::Body::Bytes(body) = listed.body else {
        panic!("notification response must be in-memory JSON");
    };
    let notifications: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(notifications.as_array().unwrap().len(), 1);
    assert_eq!(notifications[0]["id"], error.id);
    assert_eq!(notifications[0]["comments"], "Replaced the damaged media file.");

    let dismissed = service
        .resolve(&PeerRequest {
            path: format!("/notifications/device-1/{}/dismiss", error.id),
            range: None,
            if_none_match: None,
            playback: None,
            error_report: None,
            like: None,
        })
        .await;
    assert_eq!(dismissed.header.status, 204);
    assert!(library
        .list_client_resolution_notifications("device-1")
        .await
        .unwrap()
        .is_empty());
}
