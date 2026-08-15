//! Cover Art Archive client — keyless. Two-step fetch (list images, then pull
//! the one marked `front`) avoids requesting the guaranteed-404 convenience
//! redirect endpoint, mirroring Drone's `music/coverart_client.py`.

const DEFAULT_BASE: &str = "https://coverartarchive.org";

#[derive(Debug, thiserror::Error)]
pub enum CoverArtError {
    #[error("no cover art available")]
    NotFound,
    #[error("Cover Art Archive unavailable: {0}")]
    Unavailable(String),
}

pub struct CoverArtClient {
    http: reqwest::Client,
    base: String,
    /// Real Cover Art Archive responses sometimes list images as plain
    /// `http://`; upgrade to `https://` before fetching. Only applied
    /// against the real default endpoint — a caller-supplied base (tests, a
    /// self-hosted mirror) is trusted exactly as configured.
    upgrade_http: bool,
}

impl CoverArtClient {
    pub fn new() -> Self {
        Self { http: reqwest::Client::new(), base: DEFAULT_BASE.to_string(), upgrade_http: true }
    }

    pub fn with_base_url(base: impl Into<String>) -> Self {
        Self { http: reqwest::Client::new(), base: base.into(), upgrade_http: false }
    }

    pub async fn front_cover(&self, release_mbid: &str) -> Result<Vec<u8>, CoverArtError> {
        let response = self
            .http
            .get(format!("{}/release/{release_mbid}", self.base))
            .send()
            .await
            .map_err(|e| CoverArtError::Unavailable(e.to_string()))?;
        if response.status().as_u16() == 404 {
            return Err(CoverArtError::NotFound);
        }
        if !response.status().is_success() {
            return Err(CoverArtError::Unavailable(format!("listing returned {}", response.status())));
        }
        let body: ImageListResponse =
            response.json().await.map_err(|e| CoverArtError::Unavailable(e.to_string()))?;
        let front = body.images.into_iter().find(|img| img.front).ok_or(CoverArtError::NotFound)?;
        let image_url = if self.upgrade_http {
            front.image.replacen("http://", "https://", 1)
        } else {
            front.image
        };
        let image_response =
            self.http.get(&image_url).send().await.map_err(|e| CoverArtError::Unavailable(e.to_string()))?;
        if !image_response.status().is_success() {
            return Err(CoverArtError::Unavailable(format!("image fetch returned {}", image_response.status())));
        }
        image_response.bytes().await.map(|b| b.to_vec()).map_err(|e| CoverArtError::Unavailable(e.to_string()))
    }
}

impl Default for CoverArtClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(serde::Deserialize)]
struct ImageListResponse {
    #[serde(default)]
    images: Vec<ImageEntry>,
}

#[derive(serde::Deserialize)]
struct ImageEntry {
    #[serde(default)]
    front: bool,
    image: String,
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
    async fn fetches_the_front_image() {
        // Bind first so the listing route can embed the server's own address
        // as the front image's URL (front_cover downloads in two hops).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let image_url = format!("{base}/img/front.jpg");
        let router = Router::new()
            .route(
                "/release/rel-1",
                get(move || {
                    let image_url = image_url.clone();
                    async move {
                        Json(json!({"images": [
                            {"front": false, "image": "http://example.invalid/back.jpg"},
                            {"front": true, "image": image_url}
                        ]}))
                    }
                }),
            )
            .route("/img/front.jpg", get(|| async { [1u8, 2, 3, 4] }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client = CoverArtClient::with_base_url(&base);
        let bytes = client.front_cover("rel-1").await.unwrap();
        assert_eq!(bytes, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn no_front_image_is_not_found() {
        let router = Router::new().route(
            "/release/rel-2",
            get(|| async { Json(json!({"images": [{"front": false, "image": "http://x/y.jpg"}]})) }),
        );
        let base = spawn_mock(router).await;
        let client = CoverArtClient::with_base_url(&base);
        assert!(matches!(client.front_cover("rel-2").await, Err(CoverArtError::NotFound)));
    }

    #[tokio::test]
    async fn missing_release_is_not_found() {
        let router = Router::new().route(
            "/release/{mbid}",
            get(|| async { axum::http::StatusCode::NOT_FOUND }),
        );
        let base = spawn_mock(router).await;
        let client = CoverArtClient::with_base_url(&base);
        assert!(matches!(client.front_cover("nope").await, Err(CoverArtError::NotFound)));
    }
}
