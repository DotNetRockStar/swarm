//! Outbound calls to a user-configured AI provider (Claude/Codex/Grok) —
//! issue #235's "AI" tab. Every advanced feature that uses this
//! (`ai_scrape_assist` in `gui.rs`, `reorganize.rs`'s unparseable-filename
//! fallback) treats the provider as a single-turn text-completion box: send
//! a system prompt plus one user prompt, get plain text back, parse
//! whatever JSON is embedded in it. Nothing here ever writes to disk or the
//! library itself — callers own that, after a human approves.
//!
//! `model` is a plain user-editable string, not a closed enum: providers
//! ship new models faster than this app can track them, and the settings
//! UI lets a user type any model name their account has access to. The
//! defaults in `settings::default_ai_providers` are just reasonable
//! starting points.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderKind {
    Claude,
    Codex,
    Grok,
}

impl AiProviderKind {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "claude" => Some(AiProviderKind::Claude),
            "codex" => Some(AiProviderKind::Codex),
            "grok" => Some(AiProviderKind::Grok),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            AiProviderKind::Claude => "claude",
            AiProviderKind::Codex => "codex",
            AiProviderKind::Grok => "grok",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AiProviderKind::Claude => "Claude",
            AiProviderKind::Codex => "Codex",
            AiProviderKind::Grok => "Grok",
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            AiProviderKind::Claude => "https://api.anthropic.com",
            AiProviderKind::Codex => "https://api.openai.com",
            AiProviderKind::Grok => "https://api.x.ai",
        }
    }

    /// A reasonable starting point, not a guarantee the model still exists
    /// — `settings::AiProviderSetting::model` is plain user-editable text
    /// precisely because providers move faster than this app can track.
    pub fn default_model(self) -> &'static str {
        match self {
            AiProviderKind::Claude => "claude-sonnet-5",
            AiProviderKind::Codex => "gpt-5.1-codex",
            AiProviderKind::Grok => "grok-4",
        }
    }

    pub fn all() -> [AiProviderKind; 3] {
        [AiProviderKind::Claude, AiProviderKind::Codex, AiProviderKind::Grok]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("{0} request failed: {1}")]
    Http(&'static str, String),
    #[error("{0} returned an error: {1}")]
    Api(&'static str, String),
    #[error("{0} response could not be parsed: {1}")]
    Parse(&'static str, String),
}

pub struct AiClient {
    kind: AiProviderKind,
    api_key: String,
    model: String,
    base_url: String,
    http: reqwest::Client,
}

impl AiClient {
    pub fn new(kind: AiProviderKind, api_key: String, model: String) -> Self {
        Self::with_base_url(kind, api_key, model, kind.default_base_url().to_string())
    }

    pub fn with_base_url(kind: AiProviderKind, api_key: String, model: String, base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self { kind, api_key, model, base_url, http }
    }

    /// Sends a single-turn prompt and returns the model's plain-text reply.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String, AiError> {
        match self.kind {
            AiProviderKind::Claude => self.complete_anthropic(system, user).await,
            AiProviderKind::Codex | AiProviderKind::Grok => self.complete_openai_compatible(system, user).await,
        }
    }

    async fn complete_anthropic(&self, system: &str, user: &str) -> Result<String, AiError> {
        let url = format!("{}/v1/messages", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "system": system,
            "messages": [{"role": "user", "content": user}],
        });
        let response = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Http(self.kind.label(), e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| AiError::Http(self.kind.label(), e.to_string()))?;
        if !status.is_success() {
            return Err(AiError::Api(self.kind.label(), api_error_message(&text)));
        }
        #[derive(serde::Deserialize)]
        struct ContentBlock {
            text: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct MessagesResponse {
            content: Vec<ContentBlock>,
        }
        let parsed: MessagesResponse =
            serde_json::from_str(&text).map_err(|e| AiError::Parse(self.kind.label(), e.to_string()))?;
        parsed
            .content
            .into_iter()
            .find_map(|c| c.text)
            .ok_or_else(|| AiError::Parse(self.kind.label(), "empty response".to_string()))
    }

    /// OpenAI's and xAI's chat-completions endpoints are wire-compatible
    /// (same request/response shape, both accept a bearer token) — one
    /// implementation covers Codex and Grok.
    async fn complete_openai_compatible(&self, system: &str, user: &str) -> Result<String, AiError> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        });
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Http(self.kind.label(), e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| AiError::Http(self.kind.label(), e.to_string()))?;
        if !status.is_success() {
            return Err(AiError::Api(self.kind.label(), api_error_message(&text)));
        }
        #[derive(serde::Deserialize)]
        struct Message {
            content: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct Choice {
            message: Message,
        }
        #[derive(serde::Deserialize)]
        struct ChatResponse {
            choices: Vec<Choice>,
        }
        let parsed: ChatResponse =
            serde_json::from_str(&text).map_err(|e| AiError::Parse(self.kind.label(), e.to_string()))?;
        parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| AiError::Parse(self.kind.label(), "empty response".to_string()))
    }
}

/// Best-effort extraction of `{"error": {"message": "..."}}` (both
/// Anthropic's and OpenAI-compatible APIs' error shape); falls back to the
/// raw body so a genuinely different error shape is still visible to the
/// user rather than swallowed.
fn api_error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("error")?.get("message")?.as_str().map(str::to_string))
        .unwrap_or_else(|| body.to_string())
}

/// Models are asked to reply with nothing but a JSON object but don't
/// always comply (prose preamble, a wrapping code fence) — pull out the
/// first balanced-looking `{...}` span and parse just that.
pub fn parse_json_object<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;

    async fn spawn_mock(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn claude_sends_x_api_key_header_and_parses_content_text() {
        let router = Router::new().route(
            "/v1/messages",
            post(
                |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(headers.get("x-api-key").unwrap(), "secret-key");
                    assert_eq!(headers.get("anthropic-version").unwrap(), "2023-06-01");
                    assert_eq!(body["model"], "claude-sonnet-5");
                    Json(json!({"content": [{"type": "text", "text": "ok"}]}))
                },
            ),
        );
        let base = spawn_mock(router).await;
        let client = AiClient::with_base_url(
            AiProviderKind::Claude,
            "secret-key".to_string(),
            "claude-sonnet-5".to_string(),
            base,
        );
        let reply = client.complete("system", "user").await.unwrap();
        assert_eq!(reply, "ok");
    }

    #[tokio::test]
    async fn openai_compatible_sends_bearer_auth_and_parses_choice_content() {
        let router = Router::new().route(
            "/v1/chat/completions",
            post(
                |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(headers.get(axum::http::header::AUTHORIZATION).unwrap(), "Bearer secret-key");
                    assert_eq!(body["model"], "gpt-5.1-codex");
                    Json(json!({"choices": [{"message": {"content": "ok"}}]}))
                },
            ),
        );
        let base = spawn_mock(router).await;
        let client = AiClient::with_base_url(
            AiProviderKind::Codex,
            "secret-key".to_string(),
            "gpt-5.1-codex".to_string(),
            base,
        );
        let reply = client.complete("system", "user").await.unwrap();
        assert_eq!(reply, "ok");
    }

    #[tokio::test]
    async fn non_success_status_surfaces_the_provider_error_message() {
        let router = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    Json(json!({"error": {"message": "invalid API key"}})),
                )
            }),
        );
        let base = spawn_mock(router).await;
        let client = AiClient::with_base_url(
            AiProviderKind::Grok,
            "bad-key".to_string(),
            "grok-4".to_string(),
            base,
        );
        let err = client.complete("system", "user").await.unwrap_err();
        assert!(matches!(err, AiError::Api(_, message) if message == "invalid API key"));
    }

    #[test]
    fn parses_json_object_wrapped_in_prose_and_code_fences() {
        let text = "Sure, here you go:\n```json\n{\"title\": \"Heat\", \"year\": 1995}\n```\nHope that helps!";
        #[derive(serde::Deserialize)]
        struct Guess {
            title: String,
            year: u32,
        }
        let guess: Guess = parse_json_object(text).unwrap();
        assert_eq!(guess.title, "Heat");
        assert_eq!(guess.year, 1995);
    }

    #[test]
    fn returns_none_for_text_with_no_json_object() {
        assert!(parse_json_object::<serde_json::Value>("no json here").is_none());
    }
}
