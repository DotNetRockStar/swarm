//! ffprobe wrapper — captures codec/container facts at scan time so
//! direct-play decisions never have to touch the file again. ffprobe is
//! optional: absence (or any failure) degrades to "no stream info", which
//! Phase 5 treats as "must transcode to be safe" for video.

use serde::Deserialize;
use std::path::Path;
use swarm_core::peer::{AudioStreamInfo, VideoStreamInfo};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaInfo {
    pub duration_secs: Option<f64>,
    pub video: Option<VideoStreamInfo>,
    pub audio: Option<AudioStreamInfo>,
}

#[derive(Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    level: Option<i64>,
    channels: Option<u32>,
    bit_rate: Option<String>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    bit_rate: Option<String>,
}

pub async fn probe(path: &Path) -> Option<MediaInfo> {
    let output = tokio::process::Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(path)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout).ok()?;
    let mut info = MediaInfo {
        duration_secs: parsed.format.as_ref().and_then(|f| f.duration.as_deref()?.parse().ok()),
        ..Default::default()
    };
    let container_bitrate: Option<u64> = parsed.format.as_ref().and_then(|f| f.bit_rate.as_deref()?.parse().ok());
    for stream in parsed.streams {
        match stream.codec_type.as_deref() {
            Some("video") if info.video.is_none() => {
                info.video = Some(VideoStreamInfo {
                    codec: stream.codec_name.unwrap_or_default(),
                    width: stream.width.unwrap_or(0),
                    height: stream.height.unwrap_or(0),
                    // ffprobe reports H.264 level 4.1 as 41.
                    level: stream.level.map(|l| format!("{}.{}", l / 10, l % 10)),
                    bitrate: stream.bit_rate.as_deref().and_then(|b| b.parse().ok()).or(container_bitrate),
                });
            }
            Some("audio") if info.audio.is_none() => {
                info.audio = Some(AudioStreamInfo {
                    codec: stream.codec_name.unwrap_or_default(),
                    channels: stream.channels.unwrap_or(0),
                    bitrate: stream.bit_rate.as_deref().and_then(|b| b.parse().ok()),
                });
            }
            _ => {}
        }
    }
    Some(info)
}
