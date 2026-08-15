//! Device-side REST client for the SWARM STUN server: register a device with
//! a join code, join additional swarms, and fetch a swarm's device roster.
//! (WSS signaling is a separate concern, landing with the hole-punch work in
//! a later phase — this crate covers the REST half of Phase 1's contract.)

use std::collections::BTreeMap;
use swarm_core::rest::{
    ApiError, DeviceRegistration, JoinSwarmRequest, MetadataPatchRequest, RegisterDeviceRequest,
    RegisterDeviceResponse, SwarmDevicesResponse, SwarmSummary,
};

#[derive(Debug, thiserror::Error)]
pub enum StunClientError {
    #[error("could not reach STUN server: {0}")]
    Network(String),
    #[error("STUN server rejected the request ({status}, {code}): {message}")]
    Api { status: u16, code: String, message: String },
    #[error("could not parse STUN server response: {0}")]
    Decode(String),
}

impl StunClientError {
    /// True for a 401 — the caller's cue to drop the stored token and
    /// prompt for a fresh join code (the recovered Overmind's
    /// 401-then-re-register pattern).
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, StunClientError::Api { status: 401, .. })
    }
}

pub struct StunClient {
    http: reqwest::Client,
    base_url: String,
}

impl StunClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        Self { http: reqwest::Client::new(), base_url: base_url.trim_end_matches('/').to_string() }
    }

    /// Redeem a single-use join code, submitting device metadata and the
    /// device's certificate fingerprint (the TOFU trust anchor for every
    /// later peer connection).
    pub async fn register_device(
        &self,
        code: &str,
        device: DeviceRegistration,
    ) -> Result<RegisterDeviceResponse, StunClientError> {
        let request = RegisterDeviceRequest { code: code.to_string(), device };
        self.post_json("/api/v1/devices/register", &request, None).await
    }

    /// Add an already-registered device to another swarm with a fresh code.
    pub async fn join_swarm(&self, access_token: &str, code: &str) -> Result<SwarmSummary, StunClientError> {
        let request = JoinSwarmRequest { code: code.to_string() };
        self.post_json("/api/v1/swarms/join", &request, Some(access_token)).await
    }

    /// The swarm's device roster — fingerprints, online status, metadata —
    /// the source of truth for `swarm-p2p`'s `AllowedPeers` set.
    pub async fn swarm_devices(
        &self,
        access_token: &str,
        swarm_id: &str,
    ) -> Result<SwarmDevicesResponse, StunClientError> {
        self.get_json(&format!("/api/v1/swarms/{swarm_id}/devices"), Some(access_token)).await
    }

    /// Update the device's own arbitrary key/value metadata — a server uses
    /// this to self-report the address peers should dial (key
    /// `peer_addr`, value `host:port`), since the STUN roster otherwise
    /// only says a server exists, never where it is. Keys set to an empty
    /// string are removed server-side.
    pub async fn patch_metadata(
        &self,
        access_token: &str,
        device_id: &str,
        metadata: BTreeMap<String, String>,
    ) -> Result<(), StunClientError> {
        let request = MetadataPatchRequest { metadata };
        let _: serde_json::Value = self
            .patch_json(&format!("/api/v1/devices/{device_id}/metadata"), &request, Some(access_token))
            .await?;
        Ok(())
    }

    async fn post_json<Req: serde::Serialize, Resp: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
        bearer: Option<&str>,
    ) -> Result<Resp, StunClientError> {
        let mut request = self.http.post(format!("{}{path}", self.base_url)).json(body);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|e| StunClientError::Network(e.to_string()))?;
        Self::parse_response(response).await
    }

    async fn patch_json<Req: serde::Serialize, Resp: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
        bearer: Option<&str>,
    ) -> Result<Resp, StunClientError> {
        let mut request = self.http.patch(format!("{}{path}", self.base_url)).json(body);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|e| StunClientError::Network(e.to_string()))?;
        Self::parse_response(response).await
    }

    async fn get_json<Resp: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        bearer: Option<&str>,
    ) -> Result<Resp, StunClientError> {
        let mut request = self.http.get(format!("{}{path}", self.base_url));
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|e| StunClientError::Network(e.to_string()))?;
        Self::parse_response(response).await
    }

    async fn parse_response<Resp: serde::de::DeserializeOwned>(
        response: reqwest::Response,
    ) -> Result<Resp, StunClientError> {
        let status = response.status();
        if status.is_success() {
            response.json().await.map_err(|e| StunClientError::Decode(e.to_string()))
        } else {
            let status_code = status.as_u16();
            match response.json::<ApiError>().await {
                Ok(body) => Err(StunClientError::Api { status: status_code, code: body.code, message: body.message }),
                Err(_) => Err(StunClientError::Api {
                    status: status_code,
                    code: "unknown".into(),
                    message: format!("request failed with status {status_code}"),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::json;
    use swarm_core::rest::DeviceType;
    use std::collections::BTreeMap;

    async fn spawn_mock(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}")
    }

    fn sample_device() -> DeviceRegistration {
        DeviceRegistration {
            name: "Test Server".into(),
            device_type: DeviceType::Server,
            machine_id: "abc123".into(),
            cert_fingerprint: "ab".repeat(32),
            platform: "test".into(),
            app_version: "0.1.0".into(),
            metadata: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn register_device_success() {
        let router = Router::new().route(
            "/api/v1/devices/register",
            post(|| async {
                (
                    axum::http::StatusCode::CREATED,
                    Json(json!({"access_token": "tok", "device_id": "dev-1", "swarm": {"id": "sw-1", "name": "Home"}})),
                )
            }),
        );
        let base = spawn_mock(router).await;
        let client = StunClient::new(base);
        let result = client.register_device("12345678", sample_device()).await.unwrap();
        assert_eq!(result.access_token, "tok");
        assert_eq!(result.swarm.name, "Home");
    }

    #[tokio::test]
    async fn register_device_maps_api_error() {
        let router = Router::new().route(
            "/api/v1/devices/register",
            post(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    Json(json!({"code": "unauthorized", "message": "join code is invalid, expired, or already used"})),
                )
            }),
        );
        let base = spawn_mock(router).await;
        let client = StunClient::new(base);
        let err = client.register_device("00000000", sample_device()).await.unwrap_err();
        assert!(err.is_unauthorized());
        assert!(matches!(err, StunClientError::Api { code, .. } if code == "unauthorized"));
    }

    #[tokio::test]
    async fn swarm_devices_sends_bearer_token() {
        let router = Router::new().route(
            "/api/v1/swarms/sw-1/devices",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(headers.get("authorization").unwrap(), "Bearer secret-token");
                Json(json!({"swarm": {"id": "sw-1", "name": "Home"}, "devices": []}))
            }),
        );
        let base = spawn_mock(router).await;
        let client = StunClient::new(base);
        let result = client.swarm_devices("secret-token", "sw-1").await.unwrap();
        assert_eq!(result.swarm.name, "Home");
        assert!(result.devices.is_empty());
    }

    #[tokio::test]
    async fn unreachable_server_is_a_network_error() {
        // Nothing listening on this port.
        let client = StunClient::new("http://127.0.0.1:1");
        let err = client.register_device("12345678", sample_device()).await.unwrap_err();
        assert!(matches!(err, StunClientError::Network(_)));
        assert!(!err.is_unauthorized());
    }
}
