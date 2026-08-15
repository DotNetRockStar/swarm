//! MusicBrainz client — keyless, but their usage policy requires a
//! descriptive User-Agent and a ~1 request/second global rate. Ported from
//! Batocera.Drone's `music/musicbrainz_client.py`: proactive throttle (not
//! reactive backoff), release-level (not per-track) matching, two-tier
//! errors.

use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

const DEFAULT_BASE: &str = "https://musicbrainz.org/ws/2";
const USER_AGENT: &str = "SwarmMediaServer/0.1 (+https://github.com/Jerrod/swarm)";
const MIN_INTERVAL: Duration = Duration::from_millis(1050);

#[derive(Debug, thiserror::Error)]
pub enum MbError {
    #[error("no MusicBrainz match")]
    NotFound,
    #[error("MusicBrainz unavailable: {0}")]
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReleaseDetails {
    pub genres: Vec<String>,
    pub artist_mbid: Option<String>,
    pub artist_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ArtistDetails {
    /// Commons file title (e.g. `File:Some Artist 2019.jpg`) if a Wikimedia
    /// image relation exists — the input to `WikimediaClient::resolve_file_url`.
    pub commons_file: Option<String>,
}

#[derive(Clone)]
pub struct MusicBrainzClient {
    http: reqwest::Client,
    base: String,
    /// Shared across clones so every caller of one logical client serializes
    /// through the same 1 req/s gate.
    last_request: Arc<Mutex<Option<Instant>>>,
}

impl MusicBrainzClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE)
    }

    pub fn with_base_url(base: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder().user_agent(USER_AGENT).build().unwrap_or_default(),
            base: base.into(),
            last_request: Arc::new(Mutex::new(None)),
        }
    }

    async fn throttle(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < MIN_INTERVAL {
                tokio::time::sleep(MIN_INTERVAL - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    pub async fn search_release(&self, artist: &str, album: &str) -> Result<String, MbError> {
        self.throttle().await;
        let query = format!("artist:\"{}\" AND release:\"{}\"", escape_lucene(artist), escape_lucene(album));
        let response = self
            .http
            .get(format!("{}/release/", self.base))
            .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "1")])
            .send()
            .await
            .map_err(|e| MbError::Unavailable(e.to_string()))?;
        if !response.status().is_success() {
            return Err(MbError::Unavailable(format!("search returned {}", response.status())));
        }
        let body: ReleaseSearchResponse =
            response.json().await.map_err(|e| MbError::Unavailable(e.to_string()))?;
        body.releases.into_iter().next().map(|r| r.id).ok_or(MbError::NotFound)
    }

    pub async fn release_lookup(&self, release_mbid: &str) -> Result<ReleaseDetails, MbError> {
        self.throttle().await;
        let response = self
            .http
            .get(format!("{}/release/{release_mbid}", self.base))
            .query(&[("inc", "artist-credits+genres"), ("fmt", "json")])
            .send()
            .await
            .map_err(|e| MbError::Unavailable(e.to_string()))?;
        if response.status().as_u16() == 404 {
            return Err(MbError::NotFound);
        }
        if !response.status().is_success() {
            return Err(MbError::Unavailable(format!("lookup returned {}", response.status())));
        }
        let body: ReleaseLookupResponse =
            response.json().await.map_err(|e| MbError::Unavailable(e.to_string()))?;
        let first_credit = body.artist_credit.into_iter().next();
        Ok(ReleaseDetails {
            genres: body.genres.into_iter().map(|g| g.name).collect(),
            artist_mbid: first_credit.as_ref().map(|c| c.artist.id.clone()),
            artist_name: first_credit.map(|c| c.artist.name),
        })
    }

    pub async fn artist_lookup(&self, artist_mbid: &str) -> Result<ArtistDetails, MbError> {
        self.throttle().await;
        let response = self
            .http
            .get(format!("{}/artist/{artist_mbid}", self.base))
            .query(&[("inc", "url-rels"), ("fmt", "json")])
            .send()
            .await
            .map_err(|e| MbError::Unavailable(e.to_string()))?;
        if response.status().as_u16() == 404 {
            return Err(MbError::NotFound);
        }
        if !response.status().is_success() {
            return Err(MbError::Unavailable(format!("artist lookup returned {}", response.status())));
        }
        let body: ArtistLookupResponse =
            response.json().await.map_err(|e| MbError::Unavailable(e.to_string()))?;
        let commons_file = body
            .relations
            .into_iter()
            .find(|rel| rel.rel_type == "image" && rel.url.resource.contains("commons.wikimedia.org"))
            .and_then(|rel| commons_title_from_url(&rel.url.resource));
        Ok(ArtistDetails { commons_file })
    }
}

impl Default for MusicBrainzClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Lucene special characters MusicBrainz's search syntax treats specially;
/// quoting handles spaces, this handles literal quotes inside a title.
fn escape_lucene(value: &str) -> String {
    value.replace('"', "\\\"")
}

fn commons_title_from_url(url: &str) -> Option<String> {
    let decoded = url.rsplit('/').next()?.replace("_", " ");
    let decoded = urlencoding_decode(&decoded);
    if decoded.starts_with("File:") {
        Some(decoded)
    } else {
        Some(format!("File:{decoded}"))
    }
}

/// Minimal percent-decoding — Commons URLs only ever escape a small
/// character set in file titles, so a full crate is unnecessary here.
fn urlencoding_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                out.push(byte as char);
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[derive(serde::Deserialize)]
struct ReleaseSearchResponse {
    #[serde(default)]
    releases: Vec<ReleaseSearchHit>,
}

#[derive(serde::Deserialize)]
struct ReleaseSearchHit {
    id: String,
}

#[derive(serde::Deserialize)]
struct ReleaseLookupResponse {
    #[serde(default)]
    genres: Vec<MbGenre>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<ArtistCredit>,
}

#[derive(serde::Deserialize)]
struct MbGenre {
    name: String,
}

#[derive(serde::Deserialize)]
struct ArtistCredit {
    artist: ArtistRef,
}

#[derive(serde::Deserialize)]
struct ArtistRef {
    id: String,
    name: String,
}

#[derive(serde::Deserialize)]
struct ArtistLookupResponse {
    #[serde(default)]
    relations: Vec<Relation>,
}

#[derive(serde::Deserialize)]
struct Relation {
    #[serde(rename = "type")]
    rel_type: String,
    url: RelationUrl,
}

#[derive(serde::Deserialize)]
struct RelationUrl {
    resource: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Json;
    use serde_json::json;

    async fn spawn_mock(router: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn search_then_lookup() {
        let router = axum::Router::new()
            .route("/release/", get(|| async { Json(json!({"releases": [{"id": "rel-1"}]})) }))
            .route(
                "/release/rel-1",
                get(|| async {
                    Json(json!({
                        "genres": [{"name": "Rock"}],
                        "artist-credit": [{"artist": {"id": "art-1", "name": "Pink Floyd"}}]
                    }))
                }),
            );
        let base = spawn_mock(router).await;
        let client = MusicBrainzClient::with_base_url(&base);
        let mbid = client.search_release("Pink Floyd", "The Wall").await.unwrap();
        assert_eq!(mbid, "rel-1");
        let details = client.release_lookup(&mbid).await.unwrap();
        assert_eq!(details.genres, vec!["Rock"]);
        assert_eq!(details.artist_mbid.as_deref(), Some("art-1"));
    }

    #[tokio::test]
    async fn no_releases_is_not_found() {
        let router = axum::Router::new().route("/release/", get(|| async { Json(json!({"releases": []})) }));
        let base = spawn_mock(router).await;
        let client = MusicBrainzClient::with_base_url(&base);
        assert!(matches!(client.search_release("Nobody", "Nothing").await, Err(MbError::NotFound)));
    }

    #[tokio::test]
    async fn artist_lookup_finds_commons_image_relation() {
        let router = axum::Router::new().route(
            "/artist/art-1",
            get(|| async {
                Json(json!({"relations": [
                    {"type": "image", "url": {"resource": "https://commons.wikimedia.org/wiki/File:Pink_Floyd_1973.jpg"}},
                    {"type": "official homepage", "url": {"resource": "https://pinkfloyd.com"}}
                ]}))
            }),
        );
        let base = spawn_mock(router).await;
        let client = MusicBrainzClient::with_base_url(&base);
        let details = client.artist_lookup("art-1").await.unwrap();
        assert_eq!(details.commons_file.as_deref(), Some("File:Pink Floyd 1973.jpg"));
    }

    #[test]
    fn commons_title_extraction() {
        assert_eq!(
            commons_title_from_url("https://commons.wikimedia.org/wiki/File:A_B.jpg"),
            Some("File:A B.jpg".to_string())
        );
    }
}
