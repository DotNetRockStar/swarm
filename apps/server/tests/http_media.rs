//! End-to-end proof that the plain-HTTP(S) pairing + media-playback surface
//! (`http_media.rs`) actually works over a real TCP connection, not just at
//! the unit-test level — Roku-groundwork prerequisite, see the pinned
//! implementation-plan comment on GitHub issue #54. Drives the exact same
//! sequence a real HTTP-only device would: pair over HTTP, get approved via
//! the same trusted in-process call the desktop Tauri command uses, poll
//! for the bearer token, then negotiate and fetch a byte range of real
//! media — asserting on wire-level details (status codes, `Content-Range`)
//! that only a real `reqwest` round trip can actually prove.

use serde_json::{json, Value};
use std::io::Read;
use swarm_core::capability::CapabilityProfile;
use swarm_core::peer::{CatalogManifest, CatalogThumbprint, MediaKind, PlaybackMode, PlaybackPlan, VideoStreamInfo};
use swarm_media::roots::MediaRoot;
use swarm_media::store::{ArtworkKind, EntryRecord};
use swarm_server::{ServerConfig, ServerCore, TokenStoreMode};

/// Runs the same begin (device, HTTP) -> approve (owner, in-process, the
/// same trust boundary as the real Tauri command) -> poll (device, HTTP)
/// sequence every test in this file needs before it can call an
/// authenticated route, and returns the bearer token.
async fn pair_and_get_token(client: &reqwest::Client, base_url: &str, core: &ServerCore, name: &str) -> String {
    let begin: Value = client
        .post(format!("{base_url}/pair/begin"))
        .json(&json!({ "name": name }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let code = begin["code"].as_str().unwrap();
    assert_eq!(code.len(), 8);

    let (approved_name, _token_returned_to_owner) =
        core.approve_http_media_pairing(code).await.unwrap();
    assert_eq!(approved_name, name);

    let poll: Value = client
        .post(format!("{base_url}/pair/poll"))
        .json(&json!({
            "activation_id": begin["activation_id"],
            "poll_token": begin["poll_token"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(poll["status"], "approved");
    let token = poll["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 64);
    token
}

fn deterministic_bytes(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| seed.wrapping_add((i * 31 + i / 251) as u8))
        .collect()
}

#[tokio::test]
async fn pair_negotiate_and_range_fetch_media_over_real_http() {
    let base = std::env::temp_dir().join(format!("swarm-http-media-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let media_root = base.join("media");
    std::fs::create_dir_all(&media_root).unwrap();

    let config = ServerConfig {
        media_roots: vec![MediaRoot {
            label: "local".into(),
            path: media_root.clone(),
        }],
        data_dir: base.join("server-data"),
        bind: "127.0.0.1:0".parse().unwrap(),
        http_media_bind: "127.0.0.1:0".parse().unwrap(),
        allowed_fingerprints: vec![],
        token_store_mode: TokenStoreMode::FileOnly,
        managed_rendezvous_url: None,
    };
    let core = ServerCore::start(config).await.unwrap();
    // Settles the initial (here, empty-root) background scan before this
    // test's own direct upsert below, so the two can never race — same
    // reasoning `live_media_roots.rs` uses `wait_for_scan` for.
    core.wait_for_scan().await.unwrap();

    // Hardcoded codec fields, matching crates/swarm-media/tests/playback.rs's
    // established convention: this test is about the HTTP surface, not real
    // ffprobe scanning, so a direct upsert (not a real scan of a real
    // container file) is the deliberate, established way to avoid depending
    // on ffmpeg/ffprobe being installed.
    let relative_path = "movies/example.mp4";
    let media_bytes = deterministic_bytes(1_000_000, 9);
    let media_path = media_root.join(relative_path);
    std::fs::create_dir_all(media_path.parent().unwrap()).unwrap();
    std::fs::write(&media_path, &media_bytes).unwrap();
    let entry = EntryRecord {
        entry_key: "0123456789abcdef01234567".into(),
        relative_path: relative_path.into(),
        kind: MediaKind::Movie,
        title: "Example".into(),
        size: media_bytes.len() as u64,
        modified_time: 0,
        fingerprint: "fingerprint".into(),
        artist: None,
        album: None,
        track_number: None,
        show_title: None,
        season: None,
        episode: None,
        year: None,
        duration_secs: Some(10.0),
        video: Some(VideoStreamInfo {
            codec: "h264".into(),
            width: 640,
            height: 360,
            level: Some("4.1".into()),
            bitrate: Some(700_000),
        }),
        audio: None,
        scraped_title: None,
        episode_title: None,
        genres: vec![],
        artwork_version: 0,
        cast: vec![],
        overview: None,
        rating: None,
        community_rating: None,
        community_rating_votes: None,
    };
    core.library.upsert(&entry).await.unwrap();

    let base_url = format!("http://{}", core.http_media_addr);
    let client = reqwest::Client::new();

    // A browser page's fetch() carries Sec-Fetch-* metadata that a real
    // device client never sends — reject_cross_site must block it even
    // though the request's source IP genuinely is on the LAN (this test
    // runs from 127.0.0.1).
    let cross_site_attempt = client
        .post(format!("{base_url}/pair/begin"))
        .header("sec-fetch-site", "cross-site")
        .json(&json!({ "name": "Malicious Page" }))
        .send()
        .await
        .unwrap();
    assert_eq!(cross_site_attempt.status(), 403);

    let token = pair_and_get_token(&client, &base_url, &core, "Living Room Roku").await;

    // --- Auth is load-bearing: no token, no access. ---
    let unauthenticated = client
        .get(format!("{base_url}/media/{}", entry.entry_key))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), 401);

    // --- Negotiate playback with the token. ---
    let plan: PlaybackPlan = client
        .post(format!("{base_url}/play/{}", entry.entry_key))
        .bearer_auth(&token)
        .json(&json!({
            "capabilities": CapabilityProfile::fire_tv_baseline(),
            "start_position_secs": 0,
            "prefer_direct": true,
            "preview": false,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(plan.mode, PlaybackMode::Direct);
    let media_path_from_plan = plan.path.clone();

    // --- Fetch a real byte range through the negotiated path, with the
    // same token. ---
    let ranged = client
        .get(format!("{base_url}{media_path_from_plan}"))
        .bearer_auth(&token)
        .header("range", "bytes=500000-500099")
        .send()
        .await
        .unwrap();
    assert_eq!(ranged.status(), 206);
    assert_eq!(
        ranged.headers().get("content-range").unwrap(),
        "bytes 500000-500099/1000000"
    );
    let body = ranged.bytes().await.unwrap();
    assert_eq!(body.len(), 100);
    assert_eq!(&body[..], &media_bytes[500_000..500_100]);

    drop(core);
    let _ = std::fs::remove_dir_all(&base);
}

/// Proves a paired HTTP-only device can actually *browse* the library, not
/// just play an already-known entry_key — catalog/artwork share the exact
/// same `media_get` handler as the byte-serving routes (both are just
/// opaque paths to `MediaService::resolve_for_network`), so this also
/// exercises the two things that handler needs beyond plain byte-range
/// serving: forwarding the full path+query (artwork width requests ride in
/// the query string) and the If-None-Match/ETag round trip artwork caching
/// depends on.
#[tokio::test]
async fn browse_catalog_and_fetch_artwork_over_real_http() {
    let base = std::env::temp_dir().join(format!("swarm-http-catalog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let media_root = base.join("media");
    std::fs::create_dir_all(&media_root).unwrap();

    let config = ServerConfig {
        media_roots: vec![MediaRoot {
            label: "local".into(),
            path: media_root.clone(),
        }],
        data_dir: base.join("server-data"),
        bind: "127.0.0.1:0".parse().unwrap(),
        http_media_bind: "127.0.0.1:0".parse().unwrap(),
        allowed_fingerprints: vec![],
        token_store_mode: TokenStoreMode::FileOnly,
        managed_rendezvous_url: None,
    };
    let core = ServerCore::start(config).await.unwrap();
    core.wait_for_scan().await.unwrap();

    let relative_path = "movies/example.mp4";
    let media_path = media_root.join(relative_path);
    std::fs::create_dir_all(media_path.parent().unwrap()).unwrap();
    std::fs::write(&media_path, deterministic_bytes(10_000, 3)).unwrap();
    let entry = EntryRecord {
        entry_key: "fedcba9876543210fedcba9".into(),
        relative_path: relative_path.into(),
        kind: MediaKind::Movie,
        title: "Example".into(),
        size: 10_000,
        modified_time: 0,
        fingerprint: "fingerprint".into(),
        artist: None,
        album: None,
        track_number: None,
        show_title: None,
        season: None,
        episode: None,
        year: None,
        duration_secs: Some(10.0),
        video: Some(VideoStreamInfo {
            codec: "h264".into(),
            width: 640,
            height: 360,
            level: Some("4.1".into()),
            bitrate: Some(700_000),
        }),
        audio: None,
        scraped_title: None,
        episode_title: None,
        genres: vec![],
        artwork_version: 0,
        cast: vec![],
        overview: None,
        rating: None,
        community_rating: None,
        community_rating_votes: None,
    };
    core.library.upsert(&entry).await.unwrap();

    let poster_relative_path = "movies/poster.jpg";
    let poster_bytes = deterministic_bytes(2_000, 7);
    std::fs::write(media_root.join(poster_relative_path), &poster_bytes).unwrap();
    core.library
        .set_artwork(&entry.entry_key, ArtworkKind::Poster, poster_relative_path)
        .await
        .unwrap();

    let base_url = format!("http://{}", core.http_media_addr);
    let client = reqwest::Client::new();
    let token = pair_and_get_token(&client, &base_url, &core, "Living Room Roku").await;

    // --- /catalog/thumbprint ---
    let thumbprint: CatalogThumbprint = client
        .get(format!("{base_url}/catalog/thumbprint"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(thumbprint.entry_count, 1);

    // --- /catalog/manifest (plain) ---
    let manifest: CatalogManifest = client
        .get(format!("{base_url}/catalog/manifest"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(manifest.entries.len(), 1);
    assert_eq!(manifest.entries[0].entry_key, entry.entry_key);
    assert_eq!(manifest.thumbprint, thumbprint.thumbprint);

    // --- /catalog/manifest.gz — same content, compressed ---
    let gz_response = client
        .get(format!("{base_url}/catalog/manifest.gz"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(gz_response.status(), 200);
    let gz_bytes = gz_response.bytes().await.unwrap();
    let mut decompressed = String::new();
    flate2::read::GzDecoder::new(&gz_bytes[..])
        .read_to_string(&mut decompressed)
        .unwrap();
    let gz_manifest: CatalogManifest = serde_json::from_str(&decompressed).unwrap();
    assert_eq!(gz_manifest.entries[0].entry_key, entry.entry_key);

    // --- /art/{entry_key}/poster — real bytes, plus the ETag/304 round trip
    // artwork caching depends on. ---
    let art_response = client
        .get(format!("{base_url}/art/{}/poster", entry.entry_key))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(art_response.status(), 200);
    let etag = art_response
        .headers()
        .get("etag")
        .expect("art() always sets an ETag")
        .to_str()
        .unwrap()
        .to_string();
    let art_body = art_response.bytes().await.unwrap();
    assert_eq!(&art_body[..], &poster_bytes[..]);

    let cached = client
        .get(format!("{base_url}/art/{}/poster", entry.entry_key))
        .bearer_auth(&token)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        cached.status(),
        304,
        "a matching If-None-Match must short-circuit to 304, proving the ETag round trip actually works over HTTP"
    );

    drop(core);
    let _ = std::fs::remove_dir_all(&base);
}
