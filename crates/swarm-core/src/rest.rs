//! Device ↔ STUN REST contracts.
//!
//! Contract discipline (the `overmind_contract.py` lesson from the Batocera
//! federation): requests are strict (`deny_unknown_fields`) so a typo'd field
//! fails loudly at the boundary; responses are extensible (no
//! `deny_unknown_fields`) so an older device tolerates new response fields.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    /// Only requests streams from servers.
    Client,
    /// Only streams to clients.
    Server,
    /// Can do both.
    Both,
}

/// `POST /api/v1/devices/register` — redeem a single-use, swarm-scoped join
/// code. The cert fingerprint submitted here is the TOFU trust anchor for all
/// later peer connections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct RegisterDeviceRequest {
    /// 8-digit join code generated in the STUN web UI, scoped to one swarm.
    pub code: String,
    pub device: DeviceRegistration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct DeviceRegistration {
    pub name: String,
    pub device_type: DeviceType,
    /// Stable machine identity (persisted NIC-MAC-derived id), so
    /// re-registration updates the existing device row instead of duplicating.
    pub machine_id: String,
    /// Lowercase hex SHA-256 of the device's self-signed certificate (DER).
    pub cert_fingerprint: String,
    /// e.g. "macos-aarch64", "firetv-api25".
    pub platform: String,
    pub app_version: String,
    /// Arbitrary user-visible key/value metadata (hostname, cpu, disk, ...).
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RegisterDeviceResponse {
    /// Opaque bearer token; store encrypted at rest on-device.
    pub access_token: String,
    pub device_id: String,
    pub swarm: SwarmSummary,
}

/// `POST /api/v1/swarms/join` — add an already-registered device (Bearer auth)
/// to another swarm with a fresh join code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct JoinSwarmRequest {
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SwarmSummary {
    pub id: String,
    pub name: String,
}

/// One row of `GET /api/v1/swarms/{id}/devices` — everything a peer needs to
/// decide whether and how to connect. `cert_fingerprint` is the pin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SwarmDevice {
    pub device_id: String,
    pub name: String,
    pub device_type: DeviceType,
    pub cert_fingerprint: String,
    pub online: bool,
    /// RFC 3339; None until first seen.
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SwarmDevicesResponse {
    pub swarm: SwarmSummary,
    pub devices: Vec<SwarmDevice>,
}

/// Uniform error body for every non-2xx REST response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_request_roundtrip() {
        let req = RegisterDeviceRequest {
            code: "12345678".into(),
            device: DeviceRegistration {
                name: "Living Room TV".into(),
                device_type: DeviceType::Client,
                machine_id: "a1b2c3".into(),
                cert_fingerprint: "ab".repeat(32),
                platform: "firetv-api25".into(),
                app_version: "0.1.0".into(),
                metadata: BTreeMap::from([("model".into(), "AFTKA".into())]),
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<RegisterDeviceRequest>(&json).unwrap(), req);
    }

    #[test]
    fn requests_reject_unknown_fields() {
        let json = r#"{"code":"12345678","extra":true,"device":{}}"#;
        assert!(serde_json::from_str::<RegisterDeviceRequest>(json).is_err());
    }

    #[test]
    fn responses_tolerate_unknown_fields() {
        let json = r#"{"access_token":"t","device_id":"d","swarm":{"id":"s","name":"Home"},"new_field":1}"#;
        let resp: RegisterDeviceResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.swarm.name, "Home");
    }

    #[test]
    fn device_type_wire_format_is_snake_case() {
        assert_eq!(serde_json::to_string(&DeviceType::Both).unwrap(), r#""both""#);
    }
}
