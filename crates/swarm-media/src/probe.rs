//! ffprobe wrapper — captures codec/container facts at scan time so
//! direct-play decisions never have to touch the file again. ffprobe is
//! optional: absence (or any failure) degrades to "no stream info", which
//! Phase 5 treats as "must transcode to be safe" for video.

use serde::Deserialize;
use std::path::{Path, PathBuf};
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
    index: Option<usize>,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    level: Option<i64>,
    channels: Option<u32>,
    bit_rate: Option<String>,
    #[serde(default)]
    tags: FfprobeTags,
    #[serde(default)]
    disposition: FfprobeDisposition,
}

#[derive(Default, Deserialize)]
struct FfprobeTags {
    language: Option<String>,
}

#[derive(Default, Deserialize)]
struct FfprobeDisposition {
    default: Option<i32>,
}

#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    bit_rate: Option<String>,
}

pub async fn probe(path: &Path) -> Option<MediaInfo> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout).ok()?;
    let mut info = MediaInfo {
        duration_secs: parsed
            .format
            .as_ref()
            .and_then(|f| f.duration.as_deref()?.parse().ok()),
        ..Default::default()
    };
    let container_bitrate: Option<u64> = parsed
        .format
        .as_ref()
        .and_then(|f| f.bit_rate.as_deref()?.parse().ok());
    for stream in parsed.streams {
        match stream.codec_type.as_deref() {
            Some("video") if info.video.is_none() => {
                info.video = Some(VideoStreamInfo {
                    codec: stream.codec_name.unwrap_or_default(),
                    width: stream.width.unwrap_or(0),
                    height: stream.height.unwrap_or(0),
                    // ffprobe reports H.264 level 4.1 as 41.
                    level: stream.level.map(|l| format!("{}.{}", l / 10, l % 10)),
                    bitrate: stream
                        .bit_rate
                        .as_deref()
                        .and_then(|b| b.parse().ok())
                        .or(container_bitrate),
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

/// One embedded audio stream, as ffmpeg can map it (`0:{index}`) into an HLS
/// output. `is_preferred` marks the single track [select_preferred_audio_stream]
/// would pick, so the caller can flag it `default:yes` in the HLS audio group
/// without re-deriving the same English/container-default/first-track rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStreamOption {
    pub index: usize,
    pub language: Option<String>,
    pub is_preferred: bool,
}

/// Every embedded audio stream ffmpeg can map, in container order. Empty on
/// any probe failure so playback can fall back to ffmpeg's own `0:a:0`
/// selection rather than mapping nothing.
pub async fn list_audio_streams(ffmpeg_path: &Path, media_path: &Path) -> Vec<AudioStreamOption> {
    let Ok(output) = tokio::process::Command::new(ffprobe_path_for(ffmpeg_path))
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index,codec_type:stream_tags=language:stream_disposition=default",
        ])
        .arg(media_path)
        .output()
        .await
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let Ok(parsed) = serde_json::from_slice::<FfprobeOutput>(&output.stdout) else {
        return Vec::new();
    };
    let preferred = select_preferred_audio_stream(&parsed.streams);
    parsed
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .filter_map(|stream| {
            let index = stream.index?;
            Some(AudioStreamOption {
                index,
                language: stream.tags.language.clone(),
                is_preferred: Some(index) == preferred,
            })
        })
        .collect()
}

fn ffprobe_path_for(ffmpeg_path: &Path) -> PathBuf {
    let has_explicit_parent = ffmpeg_path
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty());
    if !has_explicit_parent {
        return PathBuf::from("ffprobe");
    }
    let extension = ffmpeg_path.extension();
    let name = if extension.is_some_and(|value| value.eq_ignore_ascii_case("exe")) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    };
    ffmpeg_path.with_file_name(name)
}

fn select_preferred_audio_stream(streams: &[FfprobeStream]) -> Option<usize> {
    let audio = streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"));
    audio
        .clone()
        .filter(|stream| stream.tags.language.as_deref().is_some_and(is_english))
        .max_by_key(|stream| stream.disposition.default.unwrap_or(0))
        .and_then(|stream| stream.index)
        .or_else(|| {
            audio
                .clone()
                .find(|stream| stream.disposition.default.unwrap_or(0) > 0)
                .and_then(|stream| stream.index)
        })
        .or_else(|| audio.filter_map(|stream| stream.index).next())
}

fn is_english(language: &str) -> bool {
    let normalized = language.trim().to_ascii_lowercase();
    let base = normalized.split(['-', '_']).next().unwrap_or(&normalized);
    matches!(base, "en" | "eng" | "english")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streams(json: &str) -> Vec<FfprobeStream> {
        serde_json::from_str::<FfprobeOutput>(json).unwrap().streams
    }

    #[test]
    fn english_audio_wins_over_an_earlier_default_track() {
        let parsed = streams(
            r#"{"streams":[
                {"index":1,"codec_type":"audio","tags":{"language":"jpn"},"disposition":{"default":1}},
                {"index":2,"codec_type":"audio","tags":{"language":"eng"},"disposition":{"default":0}}
            ]}"#,
        );
        assert_eq!(select_preferred_audio_stream(&parsed), Some(2));
    }

    #[test]
    fn default_then_first_are_safe_fallbacks_when_english_is_absent() {
        let with_default = streams(
            r#"{"streams":[
                {"index":1,"codec_type":"audio","tags":{"language":"jpn"},"disposition":{"default":0}},
                {"index":3,"codec_type":"audio","tags":{"language":"fra"},"disposition":{"default":1}}
            ]}"#,
        );
        let without_default = streams(
            r#"{"streams":[
                {"index":4,"codec_type":"audio"},
                {"index":5,"codec_type":"audio","tags":{"language":"deu"}}
            ]}"#,
        );
        assert_eq!(select_preferred_audio_stream(&with_default), Some(3));
        assert_eq!(select_preferred_audio_stream(&without_default), Some(4));
    }

    #[test]
    fn recognizes_common_english_language_tags() {
        for language in ["en", "eng", "English", "en-US", "en_GB"] {
            assert!(is_english(language), "did not recognize {language}");
        }
        assert!(!is_english("spa"));
    }
}
