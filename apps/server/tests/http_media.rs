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
use swarm_core::capability::CapabilityProfile;
use swarm_core::peer::{MediaKind, PlaybackMode, PlaybackPlan, VideoStreamInfo};
use swarm_media::roots::MediaRoot;
use swarm_media::store::EntryRecord;
use swarm_server::{ServerConfig, ServerCore, TokenStoreMode};

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

    // --- Pairing: begin (device, over HTTP) -> approve (owner, in-process,
    // same trust boundary as the real Tauri command) -> poll (device, over
    // HTTP) for the token. ---
    let begin: Value = client
        .post(format!("{base_url}/pair/begin"))
        .json(&json!({ "name": "Living Room Roku" }))
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
    assert_eq!(approved_name, "Living Room Roku");

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
