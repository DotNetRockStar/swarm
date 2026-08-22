//! LRCLIB client for keyless music lyrics. Exact lookup uses the track,
//! artist, album, and probed duration so similarly named recordings do not
//! silently receive one another's lyrics.

use std::time::Duration;
use swarm_core::peer::TrackLyrics;

const DEFAULT_BASE: &str = "https://lrclib.net";
const USER_AGENT: &str = "SwarmMediaServer/0.1 (+https://github.com/Jerrod/swarm)";

#[derive(Debug, thiserror::Error)]
pub enum LrclibError {
    #[error("no LRCLIB match")]
    NotFound,
    #[error("LRCLIB rate limit reached; retry after {0:?}")]
    RateLimited(Duration),
    #[error("LRCLIB unavailable: {0}")]
    Unavailable(String),
}

#[derive(Clone)]
pub struct LrclibClient {
    http: reqwest::Client,
    base: String,
}

impl LrclibClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE)
    }

    pub fn with_base_url(base: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    pub async fn lookup(
        &self,
        track_name: &str,
        artist_name: &str,
        album_name: &str,
        duration_secs: f64,
    ) -> Result<TrackLyrics, LrclibError> {
        let duration = duration_secs.round().max(1.0).to_string();
        let response = self
            .http
            .get(format!("{}/api/get", self.base))
            .query(&[
                ("track_name", track_name),
                ("artist_name", artist_name),
                ("album_name", album_name),
                ("duration", duration.as_str()),
            ])
            .send()
            .await
            .map_err(|error| LrclibError::Unavailable(error.to_string()))?;
        if response.status().as_u16() == 404 {
            return Err(LrclibError::NotFound);
        }
        if response.status().as_u16() == 429 {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or_else(|| Duration::from_secs(1));
            return Err(LrclibError::RateLimited(retry_after));
        }
        if !response.status().is_success() {
            return Err(LrclibError::Unavailable(format!(
                "lookup returned {}",
                response.status()
            )));
        }
        let result: LrclibResponse = response
            .json()
            .await
            .map_err(|error| LrclibError::Unavailable(error.to_string()))?;
        let plain_lyrics = non_empty(result.plain_lyrics);
        let synced_lyrics = non_empty(result.synced_lyrics);
        if plain_lyrics.is_none() && synced_lyrics.is_none() && !result.instrumental {
            return Err(LrclibError::NotFound);
        }
        Ok(TrackLyrics {
            provider: "lrclib".into(),
            provider_id: Some(result.id),
            language: non_empty(result.lang),
            plain_lyrics,
            synced_lyrics,
            instrumental: result.instrumental,
        })
    }
}

impl Default for LrclibClient {
    fn default() -> Self {
        Self::new()
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibResponse {
    id: i64,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    plain_lyrics: Option<String>,
    #[serde(default)]
    synced_lyrics: Option<String>,
    #[serde(default)]
    lang: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Query;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use std::collections::HashMap;

    async fn spawn_mock(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn exact_lookup_returns_synced_and_plain_lyrics() {
        let router = Router::new().route(
            "/api/get",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(
                    query.get("track_name").map(String::as_str),
                    Some("Test Song")
                );
                assert_eq!(
                    query.get("artist_name").map(String::as_str),
                    Some("Test Artist")
                );
                assert_eq!(
                    query.get("album_name").map(String::as_str),
                    Some("Test Album")
                );
                assert_eq!(query.get("duration").map(String::as_str), Some("214"));
                Json(json!({
                    "id": 17,
                    "instrumental": false,
                    "plainLyrics": "First line\nSecond line",
                    "syncedLyrics": "[00:01.00]First line\n[00:03.50]Second line",
                    "lang": "en"
                }))
            }),
        );
        let client = LrclibClient::with_base_url(spawn_mock(router).await);
        let lyrics = client
            .lookup("Test Song", "Test Artist", "Test Album", 213.7)
            .await
            .unwrap();
        assert_eq!(lyrics.provider_id, Some(17));
        assert_eq!(lyrics.language.as_deref(), Some("en"));
        assert!(lyrics
            .synced_lyrics
            .as_deref()
            .unwrap()
            .contains("Second line"));
    }

    #[tokio::test]
    async fn missing_song_is_not_found() {
        let router = Router::new().route("/api/get", get(|| async { StatusCode::NOT_FOUND }));
        let client = LrclibClient::with_base_url(spawn_mock(router).await);
        assert!(matches!(
            client.lookup("Missing", "Artist", "Album", 120.0).await,
            Err(LrclibError::NotFound)
        ));
    }

    #[tokio::test]
    async fn rate_limit_exposes_the_required_retry_delay() {
        let router = Router::new().route(
            "/api/get",
            get(|| async { (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "2")]) }),
        );
        let client = LrclibClient::with_base_url(spawn_mock(router).await);
        assert!(matches!(
            client.lookup("Song", "Artist", "Album", 120.0).await,
            Err(LrclibError::RateLimited(delay)) if delay == Duration::from_secs(2)
        ));
    }
}
