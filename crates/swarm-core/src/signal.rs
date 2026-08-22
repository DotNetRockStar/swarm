//! Device ↔ STUN signaling messages, carried as JSON text frames over the
//! persistent WSS connection (`/api/v1/ws`).
//!
//! The vocabulary is inherited from Batocera.Drone's retired mux protocol
//! (`hello`, `hello_ack`, `ping/pong`, `presence`, `signal`, `bye`, `error`);
//! the relay/transfer message types were deliberately dropped — SWARM has no
//! relay tier, and transfers are negotiated peer-to-peer over QUIC.

use crate::capability::CapabilityProfile;
use crate::rest::DeviceType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalMessage {
    /// First message after the socket opens; authenticates the connection.
    Hello {
        protocol_version: u32,
        access_token: String,
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<CapabilityProfile>,
    },
    /// Server accepts the hello. `observed_addr` is the device's public
    /// address as seen by the STUN server (TCP-derived; the UDP reflexive
    /// address still comes from the reflector).
    HelloAck {
        session_id: String,
        observed_addr: String,
        /// UDP ports the reflector is currently listening on (e.g. [443, 3478]).
        reflector_ports: Vec<u16>,
    },
    Ping {
        seq: u64,
    },
    Pong {
        seq: u64,
    },
    /// Pushed by the server to all online devices sharing a swarm whenever a
    /// peer's state changes.
    Presence {
        device_id: String,
        device_type: DeviceType,
        online: bool,
        /// Swarm ids (of those the recipient shares) this update applies to.
        swarm_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        streaming: Option<StreamingStatus>,
    },
    /// Relayed verbatim between two devices sharing a swarm; the server fills
    /// in `from` and enforces shared-swarm membership. Carries hole-punch
    /// negotiation payloads — the STUN server never opens them beyond routing.
    Signal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        to: String,
        payload: SignalPayload,
    },
    /// Graceful shutdown notice from either side.
    Bye {},
    Error {
        code: String,
        message: String,
    },
}

/// Server-app load advertisement, folded into presence so clients can score
/// sources by transcode headroom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamingStatus {
    pub transcode_capacity: u32,
    pub active_sessions: u32,
    pub hw_accel: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalPayload {
    /// Connection offer: my candidates + my cert fingerprint (re-confirming
    /// the registration-time pin) + a random punch session id.
    Offer {
        punch_id: String,
        candidates: Vec<Candidate>,
        cert_fingerprint: String,
    },
    Answer {
        punch_id: String,
        candidates: Vec<Candidate>,
        cert_fingerprint: String,
    },
    /// Mutual confirmation that PUNCH_MAGIC traffic arrived, so both sides
    /// switch to the punched 4-tuple together before the QUIC handshake.
    Punched { punch_id: String, ok: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub kind: CandidateKind,
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// Local interface address — same-LAN peers connect directly.
    Lan,
    /// Reflexive address learned from the UDP reflector.
    Reflexive,
    /// UPnP/NAT-PMP-mapped or manually forwarded port — reachable without
    /// punching.
    Forwarded,
}

/// Datagram a device sends to the UDP reflector. The reflector replies with
/// JSON `{"ip": "...", "port": ...}` — byte-compatible with the original
/// Batocera.Drone reflector contract.
pub const REFLECTOR_BIND_REQUEST: &[u8] = b"bind";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectorResponse {
    pub ip: String,
    pub port: u16,
}

/// Datagram body used during simultaneous hole punching.
pub const PUNCH_MAGIC: &[u8] = b"swarm-punch-v1";

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: &SignalMessage) {
        let json = serde_json::to_string(msg).unwrap();
        assert_eq!(&serde_json::from_str::<SignalMessage>(&json).unwrap(), msg);
    }

    #[test]
    fn all_messages_roundtrip() {
        roundtrip(&SignalMessage::Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            access_token: "tok".into(),
            device_id: "dev-1".into(),
            capabilities: Some(CapabilityProfile::fire_tv_baseline()),
        });
        roundtrip(&SignalMessage::HelloAck {
            session_id: "s".into(),
            observed_addr: "203.0.113.9:52011".into(),
            reflector_ports: vec![443, 3478],
        });
        roundtrip(&SignalMessage::Ping { seq: 7 });
        roundtrip(&SignalMessage::Pong { seq: 7 });
        roundtrip(&SignalMessage::Presence {
            device_id: "dev-2".into(),
            device_type: DeviceType::Server,
            online: true,
            swarm_ids: vec!["sw-1".into()],
            streaming: Some(StreamingStatus {
                transcode_capacity: 2,
                active_sessions: 1,
                hw_accel: true,
            }),
        });
        roundtrip(&SignalMessage::Signal {
            from: Some("dev-1".into()),
            to: "dev-2".into(),
            payload: SignalPayload::Offer {
                punch_id: "p1".into(),
                candidates: vec![
                    Candidate {
                        kind: CandidateKind::Lan,
                        ip: "192.168.1.10".into(),
                        port: 40000,
                    },
                    Candidate {
                        kind: CandidateKind::Reflexive,
                        ip: "203.0.113.9".into(),
                        port: 61234,
                    },
                ],
                cert_fingerprint: "ab".repeat(32),
            },
        });
        roundtrip(&SignalMessage::Bye {});
        roundtrip(&SignalMessage::Error {
            code: "unauthorized".into(),
            message: "bad token".into(),
        });
    }

    #[test]
    fn wire_format_uses_type_tag() {
        let json = serde_json::to_string(&SignalMessage::Ping { seq: 1 }).unwrap();
        assert_eq!(json, r#"{"type":"ping","seq":1}"#);
    }

    #[test]
    fn reflector_response_parses() {
        let resp: ReflectorResponse =
            serde_json::from_str(r#"{"ip":"203.0.113.9","port":61234}"#).unwrap();
        assert_eq!(resp.port, 61234);
    }
}
