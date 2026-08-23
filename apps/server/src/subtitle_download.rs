//! OpenSubtitles.com search/download adapter. Downloaded files are converted
//! to WebVTT and registered in the same durable subtitle table used by local
//! Whisper generation, so the existing peer playback protocol exposes both.

use std::path::Path;

use serde::{Deserialize, Serialize};
use swarm_core::peer::MediaKind;
use swarm_media::store::{EntryRecord, Library, SubtitleRecord};

const API_BASE: &str = "https://api.opensubtitles.com/api/v1";
const USER_AGENT: &str = "SWARM Media Server v0.1";

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<SearchItem>,
}

#[derive(Debug, Deserialize)]
struct SearchItem {
    attributes: SearchAttributes,
}

#[derive(Debug, Deserialize)]
struct SearchAttributes {
    #[serde(default)]
    files: Vec<SubtitleFile>,
}

#[derive(Debug, Deserialize)]
struct SubtitleFile {
    file_id: i64,
}

#[derive(Debug, Serialize)]
struct DownloadRequest {
    file_id: i64,
    sub_format: &'static str,
}

#[derive(Debug, Deserialize)]
struct DownloadResponse {
    link: String,
}

pub async fn download(
    library: &Library,
    data_dir: &Path,
    api_key: &str,
    entry_key: &str,
    language: &str,
) -> Result<SubtitleRecord, String> {
    if api_key.trim().is_empty() {
        return Err("Add an OpenSubtitles API key in Details first.".into());
    }
    if language.is_empty()
        || language.len() > 12
        || !language
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("invalid subtitle language code".into());
    }
    let entry = library
        .get(entry_key)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "media entry no longer exists".to_string())?;
    if entry.kind == MediaKind::Track {
        return Err("Downloaded subtitles are available for movies and TV episodes.".into());
    }

    let client = reqwest::Client::new();
    let query = search_query(&entry);
    let mut request = client
        .get(format!("{API_BASE}/subtitles"))
        .header("Api-Key", api_key.trim())
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .query(&[("languages", language), ("query", query.as_str())]);
    if let (Some(season), Some(episode)) = (entry.season, entry.episode) {
        request = request.query(&[
            ("season_number", season.to_string()),
            ("episode_number", episode.to_string()),
        ]);
    }
    let search = checked_json::<SearchResponse>(request.send().await, "subtitle search").await?;
    let file_id = search
        .data
        .into_iter()
        .flat_map(|item| item.attributes.files)
        .next()
        .map(|file| file.file_id)
        .ok_or_else(|| {
            format!(
                "No {language} subtitles were found for {}.",
                display_title(&entry)
            )
        })?;

    let download = checked_json::<DownloadResponse>(
        client
            .post(format!("{API_BASE}/download"))
            .header("Api-Key", api_key.trim())
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .json(&DownloadRequest {
                file_id,
                sub_format: "srt",
            })
            .send()
            .await,
        "subtitle download request",
    )
    .await?;
    let response = client
        .get(&download.link)
        .send()
        .await
        .map_err(|error| format!("subtitle file download failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "subtitle file download failed ({})",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("subtitle file download failed: {error}"))?;
    let webvtt = to_webvtt(&String::from_utf8_lossy(&bytes));
    let subtitle_dir = data_dir.join("subtitles");
    tokio::fs::create_dir_all(&subtitle_dir)
        .await
        .map_err(|error| error.to_string())?;
    let id = format!("opensubtitles-{language}");
    let final_path = subtitle_dir.join(format!("{entry_key}-{id}.vtt"));
    let partial_path = final_path.with_extension("vtt.part");
    tokio::fs::write(&partial_path, webvtt)
        .await
        .map_err(|error| error.to_string())?;
    tokio::fs::rename(&partial_path, &final_path)
        .await
        .map_err(|error| error.to_string())?;
    let record = SubtitleRecord {
        id,
        entry_key: entry.entry_key,
        language: language.to_string(),
        label: format!("{} — OpenSubtitles", language_name(language)),
        source: "opensubtitles".into(),
        format: "vtt".into(),
        file_path: final_path.to_string_lossy().into_owned(),
        fingerprint: entry.fingerprint,
    };
    library
        .upsert_subtitle(&record)
        .await
        .map_err(|error| error.to_string())?;
    Ok(record)
}

async fn checked_json<T: for<'de> Deserialize<'de>>(
    response: Result<reqwest::Response, reqwest::Error>,
    operation: &str,
) -> Result<T, String> {
    let response = response.map_err(|error| format!("{operation} failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("{operation} failed: {error}"))?;
    if !status.is_success() {
        let summary: String = body.chars().take(300).collect();
        return Err(format!("{operation} failed ({status}): {summary}"));
    }
    serde_json::from_str(&body).map_err(|error| format!("invalid {operation} response: {error}"))
}

fn display_title(entry: &EntryRecord) -> &str {
    entry.scraped_title.as_deref().unwrap_or(&entry.title)
}

fn search_query(entry: &EntryRecord) -> String {
    if entry.kind == MediaKind::Episode {
        entry
            .show_title
            .clone()
            .unwrap_or_else(|| display_title(entry).to_string())
    } else {
        display_title(entry).to_string()
    }
}

fn language_name(code: &str) -> &str {
    match code {
        "en" => "English",
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        "it" => "Italian",
        "pt-br" => "Portuguese (Brazil)",
        "pt-pt" => "Portuguese (Portugal)",
        "ja" => "Japanese",
        _ => code,
    }
}

fn to_webvtt(input: &str) -> String {
    let normalized = input.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    if normalized.trim_start().starts_with("WEBVTT") {
        return normalized;
    }
    let mut output = String::from("WEBVTT\n\n");
    for line in normalized.lines() {
        if line.contains(" --> ") {
            output.push_str(&line.replace(',', "."));
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_srt_timestamps_to_webvtt() {
        let output = to_webvtt("1\r\n00:00:01,250 --> 00:00:03,000\r\nHello\r\n");
        assert!(output.starts_with("WEBVTT\n\n"));
        assert!(output.contains("00:00:01.250 --> 00:00:03.000"));
    }
}
