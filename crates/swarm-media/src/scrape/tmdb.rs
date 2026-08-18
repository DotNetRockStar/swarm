//! TMDb client (movies + TV) — requires a user-supplied free API key, like
//! Batocera.Drone's `movies/tmdb_client.py`. Two-tier errors so one 404
//! fails one title instead of aborting a whole bulk run.

use crate::store::CastMember;
use serde::Deserialize;

const DEFAULT_API_BASE: &str = "https://api.themoviedb.org/3";
const DEFAULT_IMAGE_BASE: &str = "https://image.tmdb.org/t/p";

#[derive(Debug, Clone, thiserror::Error)]
pub enum TmdbError {
    #[error("no TMDb match")]
    NotFound,
    #[error("TMDb unavailable: {0}")]
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScrapedVideo {
    pub title: String,
    pub genres: Vec<String>,
    /// Fully-qualified image URLs, ready to download.
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    /// Top-billed cast, in TMDb's own billing order.
    pub cast: Vec<CastMember>,
}

/// A user-supplied manual TMDb match, bypassing search entirely — the
/// pinpoint-rescrape "wrong match" escape hatch. `media_type` for the
/// eventual `details_by_id` call always comes from the entry's own
/// `MediaKind` (movie vs. episode's show), not from parsing the URL, so a
/// mismatched `.../tv/...` URL pasted for a movie entry still resolves —
/// callers that want that validated should compare `resolve_id`'s success
/// against their own expectations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmdbOverride {
    /// A bare numeric TMDb id.
    Id(u64),
    /// A pasted TMDb URL, e.g. `https://www.themoviedb.org/movie/27205-inception`
    /// or `.../tv/1396-breaking-bad`. Parsed into an id at scrape time.
    Url(String),
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum TmdbOverrideError {
    #[error("could not find a TMDb id in \"{0}\"")]
    UnparseableUrl(String),
}

impl TmdbOverride {
    /// Resolve to a bare numeric id, parsing a URL if necessary. Never
    /// panics on malformed input — returns a typed error instead.
    pub fn resolve_id(&self) -> Result<u64, TmdbOverrideError> {
        match self {
            TmdbOverride::Id(id) => Ok(*id),
            TmdbOverride::Url(url) => {
                parse_tmdb_url_id(url).ok_or_else(|| TmdbOverrideError::UnparseableUrl(url.clone()))
            }
        }
    }
}

/// Extracts the numeric id from a TMDb URL shaped like
/// `.../movie/27205-inception`, `.../tv/1396-breaking-bad`, or either form
/// with no slug (`.../movie/27205`) or a trailing slash.
fn parse_tmdb_url_id(url: &str) -> Option<u64> {
    let after_host = url.split_once("themoviedb.org")?.1;
    let mut segments = after_host.trim_matches('/').split('/');
    let media_segment = segments.next()?;
    if media_segment != "movie" && media_segment != "tv" {
        return None;
    }
    let id_segment = segments.next()?;
    let digits: String = id_segment.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

pub struct TmdbClient {
    http: reqwest::Client,
    api_base: String,
    image_base: String,
    api_key: String,
}

impl TmdbClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_urls(api_key, DEFAULT_API_BASE, DEFAULT_IMAGE_BASE)
    }

    pub fn with_base_urls(api_key: impl Into<String>, api_base: impl Into<String>, image_base: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_base: api_base.into(),
            image_base: image_base.into(),
            api_key: api_key.into(),
        }
    }

    /// `year`, when known (see `EntryRecord::year` / `classify::extract_bracket_tags`),
    /// disambiguates a remake/franchise search — TMDb's `/search/movie` takes
    /// a `year` filter directly. TV search has no equivalent used here.
    pub async fn search_and_fetch_movie(&self, title: &str, year: Option<u32>) -> Result<ScrapedVideo, TmdbError> {
        let id = self.search(title, "movie", year).await?;
        self.details_by_id(id, "movie").await
    }

    pub async fn search_and_fetch_tv(&self, title: &str) -> Result<ScrapedVideo, TmdbError> {
        let id = self.search(title, "tv", None).await?;
        self.details_by_id(id, "tv").await
    }

    async fn search(&self, query: &str, media_type: &str, year: Option<u32>) -> Result<u64, TmdbError> {
        let url = format!("{}/search/{media_type}", self.api_base);
        let year_str = year.map(|y| y.to_string());
        let mut params = vec![("api_key", self.api_key.as_str()), ("query", query)];
        if media_type == "movie" {
            if let Some(year_str) = &year_str {
                params.push(("year", year_str.as_str()));
            }
        }
        let response = self
            .http
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| TmdbError::Unavailable(e.to_string()))?;
        if !response.status().is_success() {
            return Err(TmdbError::Unavailable(format!("search returned {}", response.status())));
        }
        let body: SearchResponse =
            response.json().await.map_err(|e| TmdbError::Unavailable(e.to_string()))?;
        body.results.into_iter().next().map(|hit| hit.id).ok_or(TmdbError::NotFound)
    }

    /// Fetch details for a known TMDb id directly, skipping search entirely
    /// — the manual-URL-override path (see [`TmdbOverride`]) as well as the
    /// tail end of the normal search-then-fetch flow both land here.
    pub async fn details_by_id(&self, id: u64, media_type: &str) -> Result<ScrapedVideo, TmdbError> {
        let url = format!("{}/{media_type}/{id}", self.api_base);
        let response = self
            .http
            .get(&url)
            // One request, not two: TMDb folds a sub-resource into the main
            // details payload under its own key when asked.
            .query(&[("api_key", self.api_key.as_str()), ("append_to_response", "credits")])
            .send()
            .await
            .map_err(|e| TmdbError::Unavailable(e.to_string()))?;
        if response.status().as_u16() == 404 {
            return Err(TmdbError::NotFound);
        }
        if !response.status().is_success() {
            return Err(TmdbError::Unavailable(format!("details returned {}", response.status())));
        }
        let body: DetailsResponse =
            response.json().await.map_err(|e| TmdbError::Unavailable(e.to_string()))?;
        // TMDb already orders cast by billing; keep only the headline names.
        let cast = body
            .credits
            .map(|c| c.cast.into_iter().take(10).map(|m| CastMember { name: m.name, character: m.character }).collect())
            .unwrap_or_default();
        Ok(ScrapedVideo {
            title: body.title.or(body.name).unwrap_or_default(),
            genres: body.genres.into_iter().map(|g| g.name).collect(),
            poster_url: body.poster_path.map(|p| format!("{}/w500{p}", self.image_base)),
            backdrop_url: body.backdrop_path.map(|p| format!("{}/w1280{p}", self.image_base)),
            cast,
        })
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchHit>,
}

#[derive(Deserialize)]
struct SearchHit {
    id: u64,
}

#[derive(Deserialize)]
struct DetailsResponse {
    title: Option<String>, // movies
    name: Option<String>,  // tv
    #[serde(default)]
    genres: Vec<Genre>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    /// Present because of `append_to_response=credits`.
    credits: Option<CreditsResponse>,
}

#[derive(Deserialize)]
struct Genre {
    name: String,
}

#[derive(Deserialize)]
struct CreditsResponse {
    #[serde(default)]
    cast: Vec<TmdbCastMember>,
}

#[derive(Deserialize)]
struct TmdbCastMember {
    name: String,
    character: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;

    async fn spawn_mock(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn search_and_fetch_movie_success() {
        let router = Router::new()
            .route("/search/movie", get(|| async { Json(json!({"results": [{"id": 27205}]})) }))
            .route(
                "/movie/27205",
                get(|| async {
                    Json(json!({
                        "title": "Inception", "genres": [{"id": 1, "name": "Sci-Fi"}],
                        "poster_path": "/poster.jpg", "backdrop_path": "/backdrop.jpg",
                        "credits": {"cast": [
                            {"name": "Leonardo DiCaprio", "character": "Cobb"},
                            {"name": "Ellen Page", "character": "Ariadne"}
                        ]}
                    }))
                }),
            );
        let base = spawn_mock(router).await;
        let client = TmdbClient::with_base_urls("key", &base, "https://image.tmdb.org/t/p");
        let result = client.search_and_fetch_movie("Inception", None).await.unwrap();
        assert_eq!(result.title, "Inception");
        assert_eq!(result.genres, vec!["Sci-Fi"]);
        assert_eq!(result.poster_url.as_deref(), Some("https://image.tmdb.org/t/p/w500/poster.jpg"));
        assert_eq!(result.cast.len(), 2);
        assert_eq!(result.cast[0].name, "Leonardo DiCaprio");
        assert_eq!(result.cast[0].character.as_deref(), Some("Cobb"));
    }

    #[tokio::test]
    async fn missing_credits_block_is_an_empty_cast_not_an_error() {
        let router = Router::new()
            .route("/search/movie", get(|| async { Json(json!({"results": [{"id": 1}]})) }))
            .route("/movie/1", get(|| async { Json(json!({"title": "No Credits"})) }));
        let base = spawn_mock(router).await;
        let client = TmdbClient::with_base_urls("key", &base, &base);
        let result = client.search_and_fetch_movie("No Credits", None).await.unwrap();
        assert!(result.cast.is_empty());
    }

    #[tokio::test]
    async fn empty_search_results_is_not_found() {
        let router = Router::new().route("/search/movie", get(|| async { Json(json!({"results": []})) }));
        let base = spawn_mock(router).await;
        let client = TmdbClient::with_base_urls("key", &base, &base);
        assert!(matches!(client.search_and_fetch_movie("Nope", None).await, Err(TmdbError::NotFound)));
    }

    #[tokio::test]
    async fn server_error_is_unavailable_not_not_found() {
        let router = Router::new()
            .route("/search/movie", get(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }));
        let base = spawn_mock(router).await;
        let client = TmdbClient::with_base_urls("key", &base, &base);
        assert!(matches!(client.search_and_fetch_movie("X", None).await, Err(TmdbError::Unavailable(_))));
    }

    #[tokio::test]
    async fn year_hint_is_forwarded_to_movie_search() {
        let router = Router::new()
            .route(
                "/search/movie",
                get(|axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>| async move {
                    assert_eq!(params.get("year").map(String::as_str), Some("1995"));
                    Json(json!({"results": [{"id": 949}]}))
                }),
            )
            .route("/movie/949", get(|| async { Json(json!({"title": "Heat"})) }));
        let base = spawn_mock(router).await;
        let client = TmdbClient::with_base_urls("key", &base, &base);
        let result = client.search_and_fetch_movie("Heat", Some(1995)).await.unwrap();
        assert_eq!(result.title, "Heat");
    }

    #[test]
    fn resolves_a_bare_id_override_without_parsing() {
        assert_eq!(TmdbOverride::Id(27205).resolve_id(), Ok(27205));
    }

    #[test]
    fn parses_id_from_a_full_movie_url_with_slug() {
        assert_eq!(
            TmdbOverride::Url("https://www.themoviedb.org/movie/27205-inception".into()).resolve_id(),
            Ok(27205)
        );
    }

    #[test]
    fn parses_id_from_a_tv_url_with_no_slug_and_trailing_slash() {
        assert_eq!(TmdbOverride::Url("https://www.themoviedb.org/tv/1396/".into()).resolve_id(), Ok(1396));
    }

    #[test]
    fn malformed_override_url_is_a_clean_typed_error_not_a_panic() {
        assert_eq!(
            TmdbOverride::Url("https://example.com/not-tmdb".into()).resolve_id(),
            Err(TmdbOverrideError::UnparseableUrl("https://example.com/not-tmdb".into()))
        );
        assert!(TmdbOverride::Url("https://www.themoviedb.org/movie/not-a-number".into()).resolve_id().is_err());
        assert!(TmdbOverride::Url("garbage".into()).resolve_id().is_err());
    }
}
