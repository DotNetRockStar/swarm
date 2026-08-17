//! Device ↔ device application protocol, carried over hole-punched QUIC.
//!
//! One request per QUIC bidirectional stream: the initiator writes a single
//! JSON line ([`PeerRequest`]), the responder writes a JSON line
//! ([`PeerResponseHeader`]) followed by the raw body bytes. Deliberately
//! HTTP-shaped so the loopback proxies in the TV client and the Tauri app are
//! dumb translators.

use crate::capability::CapabilityProfile;
use serde::{Deserialize, Serialize};

/// Request header line. `path` uses the fixed peer route vocabulary:
/// `/catalog/thumbprint`, `/catalog/manifest`, `/art/{entry_key}/{kind}`,
/// `/play/{entry_key}` (playback negotiation), `/stream/{session}/media`
/// (budgeted direct play), and `/hls/{session}/...` (transcode).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRequest {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<ByteRange>,
    /// Entity tag for conditional catalog/artwork fetches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_none_match: Option<String>,
    /// Present only on `/play/{entry_key}`. Keeping playback negotiation in
    /// the authenticated peer plane means the media server can make the
    /// direct/remux/transcode decision without trusting the rendezvous tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback: Option<PlaybackPreferences>,
}

/// What a player can consume and where it wants playback to begin. The
/// server intersects this with its own shared upload budget before returning
/// a [`PlaybackPlan`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybackPreferences {
    pub capabilities: CapabilityProfile,
    #[serde(default)]
    pub start_position_secs: u64,
    #[serde(default = "default_true")]
    pub prefer_direct: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    Direct,
    Hls,
}

/// Body returned by `/play/{entry_key}`. `path` is another peer path, not a
/// public URL; the client feeds it through its authenticated loopback proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybackPlan {
    pub mode: PlaybackMode,
    pub path: String,
    /// Hard server-side allocation for this session, including audio, in
    /// bits/sec. The client also uses it as its conservative ABR ceiling.
    pub max_bitrate: u64,
}

/// HTTP-style byte range. `Suffix(n)` = last `n` bytes (`bytes=-n`);
/// `FromTo` end is inclusive and `None` means "to end of file".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ByteRange {
    FromTo { start: u64, end: Option<u64> },
    Suffix { last: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerResponseHeader {
    /// HTTP-alike status: 200, 206, 304, 404, 416, 500.
    pub status: u16,
    /// Body length that follows this header line; 0 for bodyless statuses.
    pub len: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Set on 206: the satisfied range and the full entity size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_range: Option<ContentRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentRange {
    pub start: u64,
    /// Inclusive.
    pub end: u64,
    pub total: u64,
}

/// `GET /catalog/thumbprint` body — the whole-library version token. A client
/// re-syncs only when it changes (Batocera.Drone's thumbprint pattern).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogThumbprint {
    pub thumbprint: String,
    pub entry_count: u64,
}

/// `GET /catalog/manifest` body. Full listing today; `removed` supports the
/// delta form (`?since=<thumbprint>`) once servers track per-thumbprint
/// changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogManifest {
    pub thumbprint: String,
    pub entries: Vec<CatalogEntry>,
    #[serde(default)]
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Movie,
    Episode,
    Track,
}

/// One asset as advertised by a server. Clients merge manifests from all
/// servers keyed on `fingerprint` — the same bytes on two servers collapse
/// into one catalog entry with multiple sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub entry_key: String,
    /// sample-fp-v1 content identity.
    pub fingerprint: String,
    pub kind: MediaKind,
    pub title: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    // Grouping fields are path-derived (never scraped) per the Drone rule;
    // scraped titles arrive as display overlay in `scraped_title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scraped_title: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoStreamInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioStreamInfo>,
    /// Changes when artwork changes; clients cache art against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_etag: Option<String>,
}

/// ffprobe-derived stream facts captured at scan time; feeds the direct-play
/// decision without touching the file again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub codec: String,
    pub channels: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_with_range() {
        let req = PeerRequest {
            path: "/media/030fe19c72f2665e6efd018a".into(),
            range: Some(ByteRange::FromTo {
                start: 1024,
                end: None,
            }),
            if_none_match: None,
            playback: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<PeerRequest>(&json).unwrap(), req);
    }

    #[test]
    fn suffix_range_roundtrip() {
        let json = serde_json::to_string(&ByteRange::Suffix { last: 500 }).unwrap();
        assert_eq!(
            serde_json::from_str::<ByteRange>(&json).unwrap(),
            ByteRange::Suffix { last: 500 }
        );
    }

    #[test]
    fn playback_negotiation_roundtrip() {
        let request = PeerRequest {
            path: "/play/030fe19c72f2665e6efd018a".into(),
            range: None,
            if_none_match: None,
            playback: Some(PlaybackPreferences {
                capabilities: crate::capability::CapabilityProfile::fire_tv_baseline(),
                start_position_secs: 42,
                prefer_direct: true,
            }),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(serde_json::from_str::<PeerRequest>(&json).unwrap(), request);

        let plan = PlaybackPlan {
            mode: PlaybackMode::Hls,
            path: "/hls/session/master.m3u8".into(),
            max_bitrate: 4_160_000,
        };
        let json = serde_json::to_string(&plan).unwrap();
        assert_eq!(serde_json::from_str::<PlaybackPlan>(&json).unwrap(), plan);
    }

    #[test]
    fn manifest_roundtrip() {
        let manifest = CatalogManifest {
            thumbprint: "ab".repeat(32),
            entries: vec![CatalogEntry {
                entry_key: "030fe19c72f2665e6efd018a".into(),
                fingerprint: "704ac5a4284267953aab77855e0e32aa".into(),
                kind: MediaKind::Movie,
                title: "Inception".into(),
                size: 4_700_000_000,
                duration_secs: Some(8880.0),
                show_title: None,
                season: None,
                episode: None,
                artist: None,
                album: None,
                track_number: None,
                scraped_title: Some("Inception (2010)".into()),
                genres: vec!["Sci-Fi".into()],
                video: Some(VideoStreamInfo {
                    codec: "h264".into(),
                    width: 1920,
                    height: 1080,
                    level: Some("4.1".into()),
                    bitrate: Some(8_000_000),
                }),
                audio: Some(AudioStreamInfo {
                    codec: "aac".into(),
                    channels: 6,
                    bitrate: None,
                }),
                artwork_etag: Some("v1".into()),
            }],
            removed: vec![],
        };
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<CatalogManifest>(&json).unwrap(),
            manifest
        );
    }

    #[test]
    fn response_header_serializes_206() {
        let header = PeerResponseHeader {
            status: 206,
            len: 1024,
            content_type: Some("video/x-matroska".into()),
            content_range: Some(ContentRange {
                start: 0,
                end: 1023,
                total: 4_700_000_000,
            }),
            etag: None,
        };
        let json = serde_json::to_string(&header).unwrap();
        assert_eq!(
            serde_json::from_str::<PeerResponseHeader>(&json).unwrap(),
            header
        );
    }
}
