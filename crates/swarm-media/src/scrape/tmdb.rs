//! TMDb client (movies + TV) — requires a user-supplied free API key, like
//! Batocera.Drone's `movies/tmdb_client.py`. Two-tier errors so one 404
//! fails one title instead of aborting a whole bulk run.

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

    pub async fn search_and_fetch_movie(&self, title: &str) -> Result<ScrapedVideo, TmdbError> {
        let id = self.search(title, "movie").await?;
        self.details(id, "movie").await
    }

    pub async fn search_and_fetch_tv(&self, title: &str) -> Result<ScrapedVideo, TmdbError> {
        let id = self.search(title, "tv").await?;
        self.details(id, "tv").await
    }

    async fn search(&self, query: &str, media_type: &str) -> Result<u64, TmdbError> {
        let url = format!("{}/search/{media_type}", self.api_base);
        let response = self
            .http
            .get(&url)
            .query(&[("api_key", self.api_key.as_str()), ("query", query)])
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

    async fn details(&self, id: u64, media_type: &str) -> Result<ScrapedVideo, TmdbError> {
        let url = format!("{}/{media_type}/{id}", self.api_base);
        let response = self
            .http
            .get(&url)
            .query(&[("api_key", self.api_key.as_str())])
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
        Ok(ScrapedVideo {
            title: body.title.or(body.name).unwrap_or_default(),
            genres: body.genres.into_iter().map(|g| g.name).collect(),
            poster_url: body.poster_path.map(|p| format!("{}/w500{p}", self.image_base)),
            backdrop_url: body.backdrop_path.map(|p| format!("{}/w1280{p}", self.image_base)),
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
}

#[derive(Deserialize)]
struct Genre {
    name: String,
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
                        "poster_path": "/poster.jpg", "backdrop_path": "/backdrop.jpg"
                    }))
                }),
            );
        let base = spawn_mock(router).await;
        let client = TmdbClient::with_base_urls("key", &base, "https://image.tmdb.org/t/p");
        let result = client.search_and_fetch_movie("Inception").await.unwrap();
        assert_eq!(result.title, "Inception");
        assert_eq!(result.genres, vec!["Sci-Fi"]);
        assert_eq!(result.poster_url.as_deref(), Some("https://image.tmdb.org/t/p/w500/poster.jpg"));
    }

    #[tokio::test]
    async fn empty_search_results_is_not_found() {
        let router = Router::new().route("/search/movie", get(|| async { Json(json!({"results": []})) }));
        let base = spawn_mock(router).await;
        let client = TmdbClient::with_base_urls("key", &base, &base);
        assert!(matches!(client.search_and_fetch_movie("Nope").await, Err(TmdbError::NotFound)));
    }

    #[tokio::test]
    async fn server_error_is_unavailable_not_not_found() {
        let router = Router::new()
            .route("/search/movie", get(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }));
        let base = spawn_mock(router).await;
        let client = TmdbClient::with_base_urls("key", &base, &base);
        assert!(matches!(client.search_and_fetch_movie("X").await, Err(TmdbError::Unavailable(_))));
    }
}
