//! Phase 2 exit criteria, in-process over loopback QUIC: a client with a
//! pinned server fingerprint pulls the catalog, direct-plays a file with
//! byte-exact verification, and seeks via Range — while an unpinned client
//! is refused at the TLS layer.

use sha2::{Digest, Sha256};
use std::sync::Arc;
use swarm_core::peer::{ByteRange, CatalogManifest, CatalogThumbprint, MediaKind, PeerRequest};
use swarm_media::scan::scan_root;
use swarm_media::serve::{accept_loop, MediaService};
use swarm_media::store::Library;
use swarm_p2p::endpoint::{connect, listen, read_body, send_request};
use swarm_p2p::identity::{ensure_identity, DeviceIdentity};
use swarm_p2p::pin::AllowedPeers;

struct TestServer {
    addr: std::net::SocketAddr,
    identity: DeviceIdentity,
    movie_bytes: Vec<u8>,
}

fn deterministic_bytes(len: usize) -> Vec<u8> {
    // Multi-byte pattern so any offset mistake shows up in comparisons.
    (0..len).map(|i| ((i * 31 + i / 251) % 256) as u8).collect()
}

async fn spawn_test_server(tag: &str, allowed: &[&DeviceIdentity]) -> TestServer {
    let base = std::env::temp_dir().join(format!("swarm-e2e2-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let media_root = base.join("media");
    let movie_path = media_root.join("movies/Test Movie (2024)/Test.Movie.2024.mkv");
    std::fs::create_dir_all(movie_path.parent().unwrap()).unwrap();
    let movie_bytes = deterministic_bytes(3_000_000);
    std::fs::write(&movie_path, &movie_bytes).unwrap();
    let song_path = media_root.join("music/Artist/Album/01 - Song.flac");
    std::fs::create_dir_all(song_path.parent().unwrap()).unwrap();
    std::fs::write(&song_path, deterministic_bytes(90_000)).unwrap();

    let library = Library::open(base.join("library.sqlite").to_str().unwrap())
        .await
        .unwrap();
    scan_root(&library, &media_root).await.unwrap();

    let identity = ensure_identity(&base.join("server-id")).unwrap();
    let allowed_set = AllowedPeers::new();
    allowed_set.replace(allowed.iter().map(|id| id.fingerprint.clone()));
    let endpoint = listen("127.0.0.1:0".parse().unwrap(), &identity, allowed_set).unwrap();
    let addr = endpoint.local_addr().unwrap();
    let service = Arc::new(MediaService::new(Arc::new(library), media_root));
    tokio::spawn(accept_loop(endpoint, service));
    TestServer {
        addr,
        identity,
        movie_bytes,
    }
}

fn client_identity(tag: &str) -> DeviceIdentity {
    let dir = std::env::temp_dir().join(format!("swarm-e2e2-client-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    ensure_identity(&dir).unwrap()
}

fn no_range(path: &str) -> PeerRequest {
    PeerRequest {
        path: path.into(),
        range: None,
        if_none_match: None,
        playback: None,
        error_report: None,
        like: None,
    }
}

#[tokio::test]
async fn direct_play_with_seek_over_pinned_quic() {
    let client = client_identity("good");
    let server = spawn_test_server("main", &[&client]).await;
    let connection = connect(server.addr, &client, &server.identity.fingerprint)
        .await
        .unwrap();

    // Catalog: thumbprint then manifest.
    let (header, mut recv) = send_request(&connection, &no_range("/catalog/thumbprint"))
        .await
        .unwrap();
    assert_eq!(header.status, 200);
    let thumbprint: CatalogThumbprint =
        serde_json::from_slice(&read_body(&header, &mut recv).await.unwrap()).unwrap();
    assert_eq!(thumbprint.entry_count, 2);

    let (header, mut recv) = send_request(&connection, &no_range("/catalog/manifest"))
        .await
        .unwrap();
    assert_eq!(header.status, 200);
    let manifest: CatalogManifest =
        serde_json::from_slice(&read_body(&header, &mut recv).await.unwrap()).unwrap();
    assert_eq!(manifest.thumbprint, thumbprint.thumbprint);
    let movie = manifest
        .entries
        .iter()
        .find(|e| e.kind == MediaKind::Movie)
        .unwrap();
    assert_eq!(movie.size, server.movie_bytes.len() as u64);

    // Full-file direct play: byte-exact, and the etag is the sample-fp identity.
    let media_path = format!("/media/{}", movie.entry_key);
    let (header, mut recv) = send_request(&connection, &no_range(&media_path))
        .await
        .unwrap();
    assert_eq!(header.status, 200);
    assert_eq!(header.content_type.as_deref(), Some("video/x-matroska"));
    assert_eq!(header.etag.as_deref(), Some(movie.fingerprint.as_str()));
    let body = read_body(&header, &mut recv).await.unwrap();
    assert_eq!(
        Sha256::digest(&body)[..],
        Sha256::digest(&server.movie_bytes)[..]
    );

    // Seek: open-ended Range from the middle (what a player seek issues).
    let seek_offset = 1_500_000u64;
    let request = PeerRequest {
        path: media_path.clone(),
        range: Some(ByteRange::FromTo {
            start: seek_offset,
            end: None,
        }),
        if_none_match: None,
        playback: None,
        error_report: None,
        like: None,
    };
    let (header, mut recv) = send_request(&connection, &request).await.unwrap();
    assert_eq!(header.status, 206);
    let content_range = header.content_range.unwrap();
    assert_eq!(content_range.start, seek_offset);
    assert_eq!(content_range.total, server.movie_bytes.len() as u64);
    let body = read_body(&header, &mut recv).await.unwrap();
    assert_eq!(body, &server.movie_bytes[seek_offset as usize..]);

    // Bounded and suffix ranges.
    let request = PeerRequest {
        path: media_path.clone(),
        range: Some(ByteRange::FromTo {
            start: 100,
            end: Some(199),
        }),
        if_none_match: None,
        playback: None,
        error_report: None,
        like: None,
    };
    let (header, mut recv) = send_request(&connection, &request).await.unwrap();
    assert_eq!((header.status, header.len), (206, 100));
    assert_eq!(
        read_body(&header, &mut recv).await.unwrap(),
        &server.movie_bytes[100..200]
    );

    let request = PeerRequest {
        path: media_path.clone(),
        range: Some(ByteRange::Suffix { last: 64 }),
        if_none_match: None,
        playback: None,
        error_report: None,
        like: None,
    };
    let (header, mut recv) = send_request(&connection, &request).await.unwrap();
    assert_eq!((header.status, header.len), (206, 64));
    let tail = &server.movie_bytes[server.movie_bytes.len() - 64..];
    assert_eq!(read_body(&header, &mut recv).await.unwrap(), tail);

    // Error paths: past-EOF range and unknown/invalid keys.
    let request = PeerRequest {
        path: media_path,
        range: Some(ByteRange::FromTo {
            start: u64::MAX,
            end: None,
        }),
        if_none_match: None,
        playback: None,
        error_report: None,
        like: None,
    };
    let (header, _) = send_request(&connection, &request).await.unwrap();
    assert_eq!(header.status, 416);
    let (header, _) = send_request(&connection, &no_range("/media/00000000deadbeef00000000"))
        .await
        .unwrap();
    assert_eq!(header.status, 404);
    let (header, _) = send_request(&connection, &no_range("/media/../etc/passwd"))
        .await
        .unwrap();
    assert_eq!(header.status, 404);
}

#[tokio::test]
async fn unpinned_client_is_refused_at_tls() {
    let trusted = client_identity("trusted");
    let intruder = client_identity("intruder");
    let server = spawn_test_server("authz", &[&trusted]).await;

    // The intruder's fingerprint is not in the roster: the handshake (or the
    // first stream) must fail — it must never get a byte of media.
    let outcome = match connect(server.addr, &intruder, &server.identity.fingerprint).await {
        Err(_) => Err(()),
        Ok(connection) => send_request(&connection, &no_range("/catalog/thumbprint"))
            .await
            .map(|_| ())
            .map_err(|_| ()),
    };
    assert!(outcome.is_err(), "unpinned client must be rejected");

    // A client that pins the wrong server fingerprint must abort the handshake.
    let wrong_pin = "ab".repeat(32);
    assert!(connect(server.addr, &trusted, &wrong_pin).await.is_err());

    // Sanity: the trusted client with the right pin works.
    let connection = connect(server.addr, &trusted, &server.identity.fingerprint)
        .await
        .unwrap();
    let (header, _) = send_request(&connection, &no_range("/catalog/thumbprint"))
        .await
        .unwrap();
    assert_eq!(header.status, 200);
}
