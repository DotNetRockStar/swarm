//! Keyless TheIntroDB v3 client. Segment data is fetched only after TMDb has
//! identified an asset, then cached in SQLite by the scrape runner; playback
//! never depends on this public service being online.

use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use swarm_core::peer::{SkipSegment, SkipSegmentKind};
use tokio::sync::Mutex;

const DEFAULT_API_BASE: &str = "https://api.theintrodb.org/v3";
// The public endpoint allows 30 media requests per ten seconds. Stay just
// below that ceiling even when a full season has no artwork work between
// episode lookups.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum IntroDbError {
    #[error("TheIntroDB unavailable: {0}")]
    Unavailable(String),
}

pub struct IntroDbClient {
    http: reqwest::Client,
    api_base: String,
    last_request: Arc<Mutex<Option<Instant>>>,
}

impl IntroDbClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_API_BASE)
    }

    pub fn with_base_url(api_base: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_base: api_base.into().trim_end_matches('/').to_string(),
            last_request: Arc::new(Mutex::new(None)),
        }
    }

    /// Fetch every segment type for one TMDb asset. A 404 is a successful
    /// empty lookup: caching it prevents routine metadata scans from
    /// repeatedly spending the service's public request allowance.
    pub async fn segments(
        &self,
        tmdb_id: u64,
        season: Option<u32>,
        episode: Option<u32>,
        duration_secs: Option<f64>,
    ) -> Result<Vec<SkipSegment>, IntroDbError> {
        let mut query = vec![("tmdb_id", tmdb_id.to_string())];
        if let (Some(season), Some(episode)) = (season, episode) {
            query.push(("season", season.to_string()));
            query.push(("episode", episode.to_string()));
        }
        if let Some(duration_ms) = duration_secs.and_then(duration_millis) {
            query.push(("duration_ms", duration_ms.to_string()));
        }

        let mut last_request = self.last_request.lock().await;
        if let Some(wait) = last_request
            .as_ref()
            .and_then(|last| MIN_REQUEST_INTERVAL.checked_sub(last.elapsed()))
        {
            tokio::time::sleep(wait).await;
        }
        *last_request = Some(Instant::now());
        drop(last_request);

        let response = self
            .http
            .get(format!("{}/media", self.api_base))
            .query(&query)
            .send()
            .await
            .map_err(|error| IntroDbError::Unavailable(error.to_string()))?;
        if response.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        if !response.status().is_success() {
            return Err(IntroDbError::Unavailable(format!(
                "media lookup returned {}",
                response.status()
            )));
        }
        let body: MediaResponse = response
            .json()
            .await
            .map_err(|error| IntroDbError::Unavailable(error.to_string()))?;
        Ok(body.into_segments())
    }
}

fn duration_millis(seconds: f64) -> Option<u64> {
    (seconds.is_finite() && seconds > 0.0).then(|| (seconds * 1_000.0).round() as u64)
}

#[derive(Debug, Deserialize)]
struct MediaResponse {
    #[serde(default)]
    intro: Vec<ApiSegment>,
    #[serde(default)]
    recap: Vec<ApiSegment>,
    #[serde(default)]
    credits: Vec<ApiSegment>,
    #[serde(default)]
    preview: Vec<ApiSegment>,
}

impl MediaResponse {
    fn into_segments(self) -> Vec<SkipSegment> {
        let mut segments = Vec::with_capacity(
            self.intro.len() + self.recap.len() + self.credits.len() + self.preview.len(),
        );
        append_segments(&mut segments, SkipSegmentKind::Intro, self.intro);
        append_segments(&mut segments, SkipSegmentKind::Recap, self.recap);
        append_segments(&mut segments, SkipSegmentKind::Credits, self.credits);
        append_segments(&mut segments, SkipSegmentKind::Preview, self.preview);
        segments
    }
}

#[derive(Debug, Deserialize)]
struct ApiSegment {
    start_ms: Option<u64>,
    end_ms: Option<u64>,
}

fn append_segments(target: &mut Vec<SkipSegment>, kind: SkipSegmentKind, source: Vec<ApiSegment>) {
    target.extend(source.into_iter().map(|segment| SkipSegment {
        kind,
        start_ms: segment.start_ms,
        end_ms: segment.end_ms,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Query, http::StatusCode, routing::get, Json, Router};
    use serde_json::json;
    use std::collections::HashMap;

    async fn spawn_mock(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn fetches_all_segment_types_with_episode_and_duration_identity() {
        let router = Router::new().route(
            "/media",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(query.get("tmdb_id").map(String::as_str), Some("1396"));
                assert_eq!(query.get("season").map(String::as_str), Some("1"));
                assert_eq!(query.get("episode").map(String::as_str), Some("2"));
                assert_eq!(query.get("duration_ms").map(String::as_str), Some("2"));
                Json(json!({
                    "intro": [{"start_ms": null, "end_ms": 90_000}],
                    "recap": [{"start_ms": 90_000, "end_ms": 120_000}],
                    "credits": [{"start_ms": 3_000_000, "end_ms": null}],
                    "preview": [{"start_ms": 2_900_000, "end_ms": 2_950_000}]
                }))
            }),
        );
        let client = IntroDbClient::with_base_url(spawn_mock(router).await);
        let segments = client
            .segments(1396, Some(1), Some(2), Some(0.002))
            .await
            .unwrap();

        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0].kind, SkipSegmentKind::Intro);
        assert_eq!(segments[0].start_ms, None);
        assert_eq!(segments[0].end_ms, Some(90_000));
        assert_eq!(segments[3].kind, SkipSegmentKind::Preview);
    }

    #[tokio::test]
    async fn not_found_is_cached_as_an_empty_result() {
        let router = Router::new().route(
            "/media",
            get(|| async { (StatusCode::NOT_FOUND, "no data") }),
        );
        let client = IntroDbClient::with_base_url(spawn_mock(router).await);
        assert!(client
            .segments(1, None, None, None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn service_errors_remain_retryable() {
        let router = Router::new().route(
            "/media",
            get(|| async { (StatusCode::TOO_MANY_REQUESTS, "later") }),
        );
        let client = IntroDbClient::with_base_url(spawn_mock(router).await);
        assert!(matches!(
            client.segments(1, None, None, None).await,
            Err(IntroDbError::Unavailable(_))
        ));
    }
}
