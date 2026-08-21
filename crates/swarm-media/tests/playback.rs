use std::sync::Arc;
use std::time::Duration;
use swarm_core::capability::CapabilityProfile;
use swarm_core::peer::{
    AudioStreamInfo, ByteRange, MediaKind, PeerRequest, PlaybackMode, PlaybackPlan,
    PlaybackPreferences, VideoStreamInfo,
};
use swarm_media::serve::{Body, MediaService};
use swarm_media::store::{EntryRecord, Library, SubtitleRecord};
use swarm_media::transcode::TranscodeConfig;

fn request(path: String) -> PeerRequest {
    PeerRequest {
        path,
        range: None,
        if_none_match: None,
        playback: None,
        error_report: None,
        like: None,
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
        overview: None,
        rating: None,
    };
    library.upsert(&entry).await.unwrap();
    let subtitle_path = root.join("subtitles").join("example.vtt");
    std::fs::create_dir_all(subtitle_path.parent().unwrap()).unwrap();
    std::fs::write(&subtitle_path, b"WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHello\n").unwrap();
    library
        .complete_transcription(&SubtitleRecord {
            id: "whisper-en".into(),
            entry_key: entry.entry_key.clone(),
            language: "en".into(),
            label: "English — AI generated".into(),
            source: "whisper".into(),
            format: "vtt".into(),
            file_path: subtitle_path.to_string_lossy().to_string(),
            fingerprint: entry.fingerprint.clone(),
        })
        .await
        .unwrap();

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
        error_report: None,
        like: None,
    };
    let resolved = service.resolve(&negotiation).await;
    assert_eq!(resolved.header.status, 200);
    let Body::Bytes(body) = resolved.body else {
        panic!("playback plan must be JSON")
    };
    let plan: PlaybackPlan = serde_json::from_slice(&body).unwrap();
    assert_eq!(plan.mode, PlaybackMode::Direct);
    assert_eq!(plan.max_bitrate, 1_000_000);
    assert_eq!(plan.subtitles.len(), 1);
    let subtitles = service.resolve(&request(plan.subtitles[0].path.clone())).await;
    assert_eq!(subtitles.header.status, 200);
    assert_eq!(subtitles.header.content_type.as_deref(), Some("text/vtt; charset=utf-8"));
    let Body::Bytes(subtitle_body) = subtitles.body else {
        panic!("subtitle must be served as bytes")
    };
    assert!(String::from_utf8(subtitle_body).unwrap().contains("Hello"));

    let mut media_request = request(plan.path.clone());
    media_request.range = Some(ByteRange::FromTo {
        start: 500_000,
        end: Some(500_099),
    });
    let media = service.resolve(&media_request).await;
    assert_eq!((media.header.status, media.header.len), (206, 100));
    assert_eq!(service.transcode_manager().reserved_bps(), 1_000_000);

    // The first internet session already holds budget. A second internet
    // negotiation is therefore rejected, while the identical request from
    // the LAN bypasses admission control and succeeds.
    service.transcode_manager().set_max_upload_bps(1_000_000);
    let internet_retry = service.resolve(&negotiation).await;
    assert_ne!(internet_retry.header.status, 200);
    let lan_retry = service.resolve_for_network(&negotiation, true).await;
    assert_eq!(lan_retry.header.status, 200);

    // Raw file serving on LAN also omits both the global and per-session
    // byte pacers.
    let lan_media = service.resolve_for_network(&media_request, true).await;
    let Body::File { rate_limiters, .. } = lan_media.body else {
        panic!("direct media must resolve to a file")
    };
    assert!(rate_limiters.is_empty());

    let session_id = plan.path.split('/').nth(2).unwrap();
    service.transcode_manager().finish_use(session_id);
    drop(service);
    let _ = std::fs::remove_dir_all(root);
}

/// Confirmed live on real hardware: a video froze, the user pressed back,
/// and every subsequent play attempt (including of a different title) got
/// rejected with 429 "not enough upload bandwidth is available for the
/// lowest rendition" — the frozen session's reservation was never released
/// on back-press, only by the (much longer) idle timeout. `/stop/{id}`
/// exists so the client can release it immediately on its way out instead
/// of leaving the whole upload budget stuck for however long remains.
#[tokio::test]
async fn stop_releases_the_reservation_so_a_retry_no_longer_needs_the_idle_timeout() {
    let root = std::env::temp_dir().join(format!("swarm-playback-stop-{}", std::process::id()));
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
        overview: None,
        rating: None,
    };
    library.upsert(&entry).await.unwrap();

    // Budget sized to exactly fit one of this entry's 1,000,000bps direct
    // sessions and nothing more on top of it — a second concurrent
    // negotiation must fail until the first is released.
    let service = MediaService::with_transcoding(
        library,
        media_root,
        TranscodeConfig {
            enabled: true,
            ffmpeg_path: "ffmpeg".into(),
            session_dir: root.join("sessions"),
            max_upload_bps: 1_000_000,
            reserve_percent: 0,
            max_sessions: 1,
            idle_timeout: Duration::from_secs(300),
            segment_duration_secs: 4,
        },
    );
    let negotiate = || PeerRequest {
        path: format!("/play/{}", entry.entry_key),
        range: None,
        if_none_match: None,
        playback: Some(PlaybackPreferences {
            capabilities: CapabilityProfile::fire_tv_baseline(),
            start_position_secs: 0,
            prefer_direct: true,
        }),
        error_report: None,
        like: None,
    };

    let first = service.resolve(&negotiate()).await;
    assert_eq!(first.header.status, 200);
    let Body::Bytes(body) = first.body else {
        panic!("playback plan must be JSON")
    };
    let plan: PlaybackPlan = serde_json::from_slice(&body).unwrap();
    assert_eq!(plan.mode, PlaybackMode::Direct);

    // Simulates "froze, pressed back, tried to play again" without ever
    // calling /stop: the reservation is still held, so this must still fail.
    let stuck_retry = service.resolve(&negotiate()).await;
    assert_eq!(
        stuck_retry.header.status, 429,
        "budget must still be held by the first, unreleased session"
    );

    let stop = service
        .resolve(&request(format!("/stop/{}", plan.session_id)))
        .await;
    assert_eq!(stop.header.status, 200);
    assert_eq!(
        service.transcode_manager().reserved_bps(),
        0,
        "release must free the whole reservation immediately, not just mark it idle"
    );

    let retry_after_stop = service.resolve(&negotiate()).await;
    assert_eq!(
        retry_after_stop.header.status, 200,
        "retry must succeed right away now, not after waiting out idle_timeout"
    );

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
            overview: None,
            rating: None,
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
            error_report: None,
            like: None,
        };
        let resolved = service.resolve(&negotiation).await;
        assert_eq!(
            resolved.header.status, 200,
            "negotiation for {entry_key} must not be capacity-limited"
        );
        let Body::Bytes(body) = resolved.body else {
            panic!("playback plan must be JSON")
        };
        let plan: PlaybackPlan = serde_json::from_slice(&body).unwrap();
        assert_eq!(plan.mode, PlaybackMode::Direct);
    }

    drop(service);
    let _ = std::fs::remove_dir_all(root);
}
