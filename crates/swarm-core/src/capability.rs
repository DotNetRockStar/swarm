//! Client playback capability profile, sent once per session. Servers use it
//! to decide direct play vs. transcode and to prune the ABR ladder.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CapabilityProfile {
    /// Container formats the client can demux, e.g. `["mp4", "mkv", "hls"]`.
    pub containers: Vec<String>,
    /// Video codecs with an optional profile/level suffix the client decodes
    /// in hardware, e.g. `["h264:high@4.2", "hevc:main@5.1"]`.
    pub video_codecs: Vec<String>,
    /// Audio codecs, e.g. `["aac", "ac3", "flac", "mp3", "opus"]`.
    pub audio_codecs: Vec<String>,
    pub max_width: u32,
    pub max_height: u32,
    /// Sustained bitrate ceiling in bits/sec the client wants to receive; the
    /// server caps the top ABR rung at this.
    pub max_bitrate: u64,
    #[serde(default)]
    pub hdr: bool,
}

impl CapabilityProfile {
    /// Baseline every Fire OS 6+ device satisfies; used before the client has
    /// probed its own decoders.
    pub fn fire_tv_baseline() -> Self {
        Self {
            containers: vec!["mp4".into(), "hls".into()],
            video_codecs: vec!["h264:high@4.2".into()],
            audio_codecs: vec!["aac".into(), "ac3".into(), "mp3".into()],
            max_width: 1920,
            max_height: 1080,
            max_bitrate: 12_000_000,
            hdr: false,
        }
    }
}
