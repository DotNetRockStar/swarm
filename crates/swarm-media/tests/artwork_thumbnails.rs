use std::sync::Arc;
use swarm_core::peer::{MediaKind, PeerRequest};
use swarm_media::serve::{Body, MediaService};
use swarm_media::store::{ArtworkKind, EntryRecord, Library};

fn request(path: &str) -> PeerRequest {
    PeerRequest {
        path: path.into(),
        range: None,
        if_none_match: None,
        playback: None,
        error_report: None,
        like: None,
    }
}

fn entry(entry_key: &str) -> EntryRecord {
    EntryRecord {
        entry_key: entry_key.into(),
        relative_path: "movies/example.mp4".into(),
        kind: MediaKind::Movie,
        title: "Example".into(),
        size: 1,
        modified_time: 0,
        fingerprint: "fingerprint".into(),
        artist: None,
        album: None,
        track_number: None,
        show_title: None,
        season: None,
        episode: None,
        year: None,
        duration_secs: None,
        video: None,
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
    }
}

#[tokio::test]
async fn card_artwork_uses_a_persistent_versioned_thumbnail() {
    let root =
        std::env::temp_dir().join(format!("swarm-artwork-thumbnail-{}", rand::random::<u64>()));
    let media_root = root.join("media");
    let images = media_root.join("movies/images");
    std::fs::create_dir_all(&images).unwrap();
    let original = images.join("poster.png");
    image::RgbImage::from_pixel(1200, 1800, image::Rgb([25, 75, 125]))
        .save(&original)
        .unwrap();

    let library = Arc::new(
        Library::open(root.join("library.sqlite").to_str().unwrap())
            .await
            .unwrap(),
    );
    let entry_key = "0123456789abcdef01234567";
    library.upsert(&entry(entry_key)).await.unwrap();
    library
        .set_artwork(entry_key, ArtworkKind::Poster, "movies/images/poster.png")
        .await
        .unwrap();
    let service = MediaService::new(library, media_root);

    let first = service
        .resolve(&request(&format!("/art/{entry_key}/poster?v=v1&w=320")))
        .await;
    assert_eq!(first.header.status, 200);
    assert_eq!(first.header.etag.as_deref(), Some("v1-w320"));
    assert_eq!(first.header.content_type.as_deref(), Some("image/jpeg"));
    let Body::File {
        path: first_path, ..
    } = first.body
    else {
        panic!("thumbnail should be file-backed")
    };
    let thumbnail = image::open(&first_path).unwrap();
    assert_eq!((thumbnail.width(), thumbnail.height()), (320, 480));

    let second = service
        .resolve(&request(&format!("/art/{entry_key}/poster?v=v1&w=320")))
        .await;
    let Body::File {
        path: second_path, ..
    } = second.body
    else {
        panic!("cached thumbnail should be file-backed")
    };
    assert_eq!(second_path, first_path);

    let full = service
        .resolve(&request(&format!("/art/{entry_key}/poster?v=v1")))
        .await;
    let Body::File {
        path: full_path, ..
    } = full.body
    else {
        panic!("full artwork should be file-backed")
    };
    assert_eq!(full_path, original);

    let _ = std::fs::remove_dir_all(root);
}
