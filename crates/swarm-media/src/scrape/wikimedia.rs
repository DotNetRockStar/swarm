//! Wikimedia Commons client — resolves a Commons file title (learned from a
//! MusicBrainz artist's `image` relation) to the real `upload.wikimedia.org`
//! URL via the MediaWiki Action API. Ported from Drone's
//! `music/wikimedia_client.py`.

const DEFAULT_BASE: &str = "https://commons.wikimedia.org/w/api.php";

#[derive(Debug, thiserror::Error)]
pub enum WikimediaError {
    #[error("file not found on Commons")]
    NotFound,
    #[error("Wikimedia unavailable: {0}")]
    Unavailable(String),
}

pub struct WikimediaClient {
    http: reqwest::Client,
    base: String,
}

impl WikimediaClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE)
    }

    pub fn with_base_url(base: impl Into<String>) -> Self {
        Self { http: reqwest::Client::new(), base: base.into() }
    }

    /// `file_title` like `File:Some Artist 2019.jpg`.
    pub async fn resolve_file_url(&self, file_title: &str) -> Result<String, WikimediaError> {
        let response = self
            .http
            .get(&self.base)
            .query(&[
                ("action", "query"),
                ("titles", file_title),
                ("prop", "imageinfo"),
                ("iiprop", "url"),
                ("format", "json"),
            ])
            .send()
            .await
            .map_err(|e| WikimediaError::Unavailable(e.to_string()))?;
        if !response.status().is_success() {
            return Err(WikimediaError::Unavailable(format!("query returned {}", response.status())));
        }
        let body: QueryResponse =
            response.json().await.map_err(|e| WikimediaError::Unavailable(e.to_string()))?;
        body.query
            .pages
            .into_values()
            .find_map(|page| page.imageinfo.into_iter().next().map(|info| info.url))
            .ok_or(WikimediaError::NotFound)
    }

    pub async fn download(&self, url: &str) -> Result<Vec<u8>, WikimediaError> {
        let response = self.http.get(url).send().await.map_err(|e| WikimediaError::Unavailable(e.to_string()))?;
        if !response.status().is_success() {
            return Err(WikimediaError::Unavailable(format!("download returned {}", response.status())));
        }
        response.bytes().await.map(|b| b.to_vec()).map_err(|e| WikimediaError::Unavailable(e.to_string()))
    }
}

impl Default for WikimediaClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(serde::Deserialize)]
struct QueryResponse {
    query: QueryBody,
}

#[derive(serde::Deserialize)]
struct QueryBody {
    pages: std::collections::HashMap<String, Page>,
}

#[derive(serde::Deserialize)]
struct Page {
    #[serde(default)]
    imageinfo: Vec<ImageInfo>,
}

#[derive(serde::Deserialize)]
struct ImageInfo {
    url: String,
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
    async fn resolves_the_image_url() {
        let router = Router::new().route(
            "/",
            get(|| async {
                Json(json!({"query": {"pages": {"123": {"imageinfo": [
                    {"url": "https://upload.wikimedia.org/commons/a/ab/Pink_Floyd_1973.jpg"}
                ]}}}}))
            }),
        );
        let base = spawn_mock(router).await;
        let client = WikimediaClient::with_base_url(&base);
        let url = client.resolve_file_url("File:Pink Floyd 1973.jpg").await.unwrap();
        assert_eq!(url, "https://upload.wikimedia.org/commons/a/ab/Pink_Floyd_1973.jpg");
    }

    #[tokio::test]
    async fn missing_page_is_not_found() {
        let router = Router::new()
            .route("/", get(|| async { Json(json!({"query": {"pages": {"-1": {}}}})) }));
        let base = spawn_mock(router).await;
        let client = WikimediaClient::with_base_url(&base);
        assert!(matches!(client.resolve_file_url("File:Nope.jpg").await, Err(WikimediaError::NotFound)));
    }
}
