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
