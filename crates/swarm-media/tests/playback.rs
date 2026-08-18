use std::sync::Arc;
use std::time::Duration;
use swarm_core::capability::CapabilityProfile;
use swarm_core::peer::{
    AudioStreamInfo, ByteRange, MediaKind, PeerRequest, PlaybackMode, PlaybackPlan,
    PlaybackPreferences, VideoStreamInfo,
};
use swarm_media::serve::{Body, MediaService};
use swarm_media::store::{EntryRecord, Library};
use swarm_media::transcode::TranscodeConfig;

fn request(path: String) -> PeerRequest {
    PeerRequest {
        path,
        range: None,
        if_none_match: None,
        playback: None,
    }
}

#[tokio::test]
async fn playback_negotiation_returns_a_budgeted_direct_session_with_range_support() {
    let root = std::env::temp_dir().join(format!("swarm-playback-route-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let media_root = root.join("media");
    std::fs::create_dir_all(&media_root).unwrap();
    let relative_path = "movies/example.mp4";
    let media_path = media_root.join(relative_path);
    std::fs::create_dir_all(media_path.parent().unwrap()).unwrap();
    std::fs::write(&media_path, vec![7u8; 1_000_000]).unwrap();

    let library = Arc::new(
        Library::open(root.join("library.sqlite").to_str().unwrap())
            .await
            .unwrap(),
    );
    let entry = EntryRecord {
        entry_key: "0123456789abcdef01234567".into(),
        relative_path: relative_path.into(),
        kind: MediaKind::Movie,
        title: "Example".into(),
        size: 1_000_000,
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
        audio: Some(AudioStreamInfo {
            codec: "aac".into(),
            channels: 2,
            bitrate: Some(96_000),
        }),
        scraped_title: None,
        genres: vec![],
        artwork_version: 0,
        cast: vec![],
    };
    library.upsert(&entry).await.unwrap();

    let service = MediaService::with_transcoding(
        library,
        media_root,
        TranscodeConfig {
            enabled: false,
            ffmpeg_path: "ffmpeg".into(),
            session_dir: root.join("sessions"),
            max_upload_bps: 10_000_000,
            reserve_percent: 30,
            max_sessions: 1,
            idle_timeout: Duration::from_secs(300),
            segment_duration_secs: 4,
        },
    );
    let negotiation = PeerRequest {
        path: format!("/play/{}", entry.entry_key),
        range: None,
        if_none_match: None,
        playback: Some(PlaybackPreferences {
            capabilities: CapabilityProfile::fire_tv_baseline(),
            start_position_secs: 0,
            prefer_direct: true,
        }),
    };
    let resolved = service.resolve(&negotiation).await;
    assert_eq!(resolved.header.status, 200);
    let Body::Bytes(body) = resolved.body else {
        panic!("playback plan must be JSON")
    };
    let plan: PlaybackPlan = serde_json::from_slice(&body).unwrap();
    assert_eq!(plan.mode, PlaybackMode::Direct);
    assert_eq!(plan.max_bitrate, 1_000_000);

    let mut media_request = request(plan.path.clone());
    media_request.range = Some(ByteRange::FromTo {
        start: 500_000,
        end: Some(500_099),
    });
    let media = service.resolve(&media_request).await;
    assert_eq!((media.header.status, media.header.len), (206, 100));
    assert_eq!(service.transcode_manager().reserved_bps(), 1_000_000);

    let session_id = plan.path.split('/').nth(2).unwrap();
    service.transcode_manager().finish_use(session_id);
    drop(service);
    let _ = std::fs::remove_dir_all(root);
}

/// Confirmed live on real hardware: with two lingering direct-play sessions
/// from earlier, unrelated plays (neither expired yet — they sit for up to
/// `idle_timeout` after their last byte, not released the moment playback
/// actually stops), negotiating a plain direct-play-eligible music file
/// failed with 429 "server transcode capacity is full" — max_sessions=1 was
/// being spent by sessions that never spawned an ffmpeg process at all.
/// Direct play is bandwidth-limited, not process-limited, so N of them must
/// be able to coexist under a max_sessions of 1 as long as the (separately
/// enforced, still-real) bandwidth budget allows it.
#[tokio::test]
async fn direct_play_sessions_are_not_limited_by_max_sessions() {
    let root = std::env::temp_dir().join(format!("swarm-playback-capacity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let media_root = root.join("media");
    std::fs::create_dir_all(&media_root).unwrap();

    let library = Arc::new(
        Library::open(root.join("library.sqlite").to_str().unwrap())
            .await
            .unwrap(),
    );
    let mut entry_keys = Vec::new();
    for i in 0..2 {
        // .m4a (not .mp3): direct_compatible() only allows containers the
        // client's profile lists, and "mp3" isn't one of them (confirmed
        // live — real .mp3 files always go through HLS) — that's a real,
        // separate, correct behavior this test isn't about, so use a
        // container that direct-play actually accepts.
        let relative_path = format!("music/track{i}.m4a");
        let media_path = media_root.join(&relative_path);
        std::fs::create_dir_all(media_path.parent().unwrap()).unwrap();
        std::fs::write(&media_path, vec![7u8; 10_000]).unwrap();
        let entry_key = format!("0123456789abcdef0123456{i}");
        let entry = EntryRecord {
            entry_key: entry_key.clone(),
            relative_path,
            kind: MediaKind::Track,
            title: format!("Track {i}"),
            size: 10_000,
            modified_time: 0,
            fingerprint: format!("fingerprint{i}"),
            artist: None,
            album: None,
            track_number: None,
            show_title: None,
            season: None,
            episode: None,
            year: None,
            duration_secs: Some(180.0),
            video: None,
            audio: Some(AudioStreamInfo {
                codec: "aac".into(),
                channels: 2,
                bitrate: Some(128_000),
            }),
            scraped_title: None,
            genres: vec![],
            artwork_version: 0,
            cast: vec![],
        };
        library.upsert(&entry).await.unwrap();
        entry_keys.push(entry_key);
    }

    let service = MediaService::with_transcoding(
        library,
        media_root,
        TranscodeConfig {
            enabled: true,
            ffmpeg_path: "ffmpeg".into(),
            session_dir: root.join("sessions"),
            max_upload_bps: 10_000_000,
            reserve_percent: 30,
            max_sessions: 1,
            idle_timeout: Duration::from_secs(300),
            segment_duration_secs: 4,
        },
    );

    for entry_key in &entry_keys {
        let negotiation = PeerRequest {
            path: format!("/play/{entry_key}"),
            range: None,
            if_none_match: None,
            playback: Some(PlaybackPreferences {
                capabilities: CapabilityProfile::fire_tv_baseline(),
                start_position_secs: 0,
                prefer_direct: true,
            }),
        };
        let resolved = service.resolve(&negotiation).await;
        assert_eq!(resolved.header.status, 200, "negotiation for {entry_key} must not be capacity-limited");
        let Body::Bytes(body) = resolved.body else {
            panic!("playback plan must be JSON")
        };
        let plan: PlaybackPlan = serde_json::from_slice(&body).unwrap();
        assert_eq!(plan.mode, PlaybackMode::Direct);
    }

    drop(service);
    let _ = std::fs::remove_dir_all(root);
}
