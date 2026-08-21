//! `/likes/toggle` and the `entry_likes` store — a device marking an asset
//! liked/unliked, aggregated into `CatalogEntry::like_count` on the next
//! `/catalog/manifest` fetch.

use std::path::Path;
use std::sync::Arc;
use swarm_core::peer::{LikeToggle, PeerRequest};
use swarm_media::scan::scan_root;
use swarm_media::serve::MediaService;
use swarm_media::store::Library;

fn toggle_request(like: Option<LikeToggle>) -> PeerRequest {
    PeerRequest { path: "/likes/toggle".into(), range: None, if_none_match: None, playback: None, error_report: None, like }
}

fn manifest_request() -> PeerRequest {
    PeerRequest { path: "/catalog/manifest".into(), range: None, if_none_match: None, playback: None, error_report: None, like: None }
}

fn sample_like(entry_key: &str, liked: bool) -> LikeToggle {
    LikeToggle { device_id: "device-1".into(), device_name: "Living Room Fire TV".into(), entry_key: entry_key.into(), liked }
}

fn write(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

// Same uniqueness reasoning as `client_errors.rs`'s `rand_suffix` — parallel
// `#[tokio::test]`s must not collide on the same temp dir/SQLite file.
fn rand_suffix() -> u128 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    now + n as u128
}

struct Fixture {
    library: Arc<Library>,
    service: MediaService,
    entry_key: String,
}

async fn fixture() -> Fixture {
    let base = std::env::temp_dir().join(format!("swarm-likes-{}-{}", std::process::id(), rand_suffix()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("media");
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "movies/Heat (1995)/Heat.1995.mkv", &[1u8; 10]);

    let library = Library::open(base.join("library.sqlite").to_str().unwrap()).await.unwrap();
    scan_root(&library, &root).await.unwrap();
    let entry = library.list().await.unwrap().into_iter().next().unwrap();

    let library = Arc::new(library);
    let service = MediaService::new(library.clone(), root);
    Fixture { library, service, entry_key: entry.entry_key }
}

#[tokio::test]
async fn liking_persists_and_counts_toward_the_manifest() {
    let fx = fixture().await;

    let response = fx.service.resolve(&toggle_request(Some(sample_like(&fx.entry_key, true)))).await;
    assert_eq!(response.header.status, 204);

    let counts = fx.library.like_counts().await.unwrap();
    assert_eq!(counts.get(&fx.entry_key).copied(), Some(1));

    // `to_catalog_entry` alone never knows the count (see its own doc
    // comment) — real manifest assembly happens in `serve.rs::manifest`,
    // exercised here via the resolved `/catalog/manifest` route.
    let manifest_response = fx.service.resolve(&manifest_request()).await;
    assert_eq!(manifest_response.header.status, 200);
    let swarm_media::serve::Body::Bytes(body) = manifest_response.body else { panic!("expected an in-memory body") };
    let manifest: swarm_core::peer::CatalogManifest = serde_json::from_slice(&body).unwrap();
    let manifest_entry = manifest.entries.iter().find(|e| e.entry_key == fx.entry_key).unwrap();
    assert_eq!(manifest_entry.like_count, 1);
}

#[tokio::test]
async fn unliking_removes_the_like_and_is_idempotent() {
    let fx = fixture().await;
    fx.service.resolve(&toggle_request(Some(sample_like(&fx.entry_key, true)))).await;
    assert_eq!(fx.library.like_counts().await.unwrap().get(&fx.entry_key).copied(), Some(1));

    // Two unlikes in a row (simulating a retried D-pad toggle) must not error
    // or go negative — see `LikeToggle`'s doc comment on desired-end-state
    // semantics.
    let first = fx.service.resolve(&toggle_request(Some(sample_like(&fx.entry_key, false)))).await;
    assert_eq!(first.header.status, 204);
    let second = fx.service.resolve(&toggle_request(Some(sample_like(&fx.entry_key, false)))).await;
    assert_eq!(second.header.status, 204);

    assert_eq!(fx.library.like_counts().await.unwrap().get(&fx.entry_key), None);
}

#[tokio::test]
async fn two_devices_liking_the_same_entry_both_count() {
    let fx = fixture().await;
    fx.service.resolve(&toggle_request(Some(sample_like(&fx.entry_key, true)))).await;
    let mut second_device = sample_like(&fx.entry_key, true);
    second_device.device_id = "device-2".into();
    second_device.device_name = "Bedroom Fire TV".into();
    fx.service.resolve(&toggle_request(Some(second_device))).await;

    assert_eq!(fx.library.like_counts().await.unwrap().get(&fx.entry_key).copied(), Some(2));
}

#[tokio::test]
async fn request_with_no_like_body_is_rejected() {
    let fx = fixture().await;
    let response = fx.service.resolve(&toggle_request(None)).await;
    assert_eq!(response.header.status, 400);
}

#[tokio::test]
async fn request_with_an_empty_device_id_or_entry_key_is_rejected() {
    let fx = fixture().await;

    let mut missing_device = sample_like(&fx.entry_key, true);
    missing_device.device_id = String::new();
    assert_eq!(fx.service.resolve(&toggle_request(Some(missing_device))).await.header.status, 400);

    let mut missing_entry_key = sample_like(&fx.entry_key, true);
    missing_entry_key.entry_key = String::new();
    assert_eq!(fx.service.resolve(&toggle_request(Some(missing_entry_key))).await.header.status, 400);

    assert!(fx.library.like_counts().await.unwrap().is_empty());
}

#[tokio::test]
async fn re_liking_an_already_liked_entry_does_not_double_count() {
    let fx = fixture().await;
    fx.service.resolve(&toggle_request(Some(sample_like(&fx.entry_key, true)))).await;
    fx.service.resolve(&toggle_request(Some(sample_like(&fx.entry_key, true)))).await;
    assert_eq!(fx.library.like_counts().await.unwrap().get(&fx.entry_key).copied(), Some(1));
}
