//! Device-side WSS signaling client (`docs/PROTOCOL.md`'s "Signaling
//! session"): the persistent connection a device holds open to receive
//! presence updates and relay hole-punch negotiation (`signal`) with its
//! swarm-mates. Separate from [`crate::client::StunClient`] (the REST half)
//! because the two have entirely different connection lifecycles — one
//! request/response call each, one long-lived duplex stream.

use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use swarm_core::capability::CapabilityProfile;
use swarm_core::signal::SignalMessage;
use swarm_core::PROTOCOL_VERSION;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How often this client sends `Ping` — signaling is client-driven per
/// `docs/PROTOCOL.md` ("`ping`/`pong` keepalive (client-driven, ~30s)").
const PING_INTERVAL: Duration = Duration::from_secs(30);
const HELLO_ACK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum SignalingError {
    #[error("SWARM server URL must start with http:// or https://, got: {0}")]
    InvalidBaseUrl(String),
    #[error("could not connect to the signaling endpoint: {0}")]
    Connect(String),
    #[error("timed out waiting for hello_ack")]
    HelloTimeout,
    #[error("connection closed before hello_ack")]
    ConnectionClosed,
    #[error("server rejected hello ({code}): {message}")]
    Rejected { code: String, message: String },
    #[error("unexpected frame from server during handshake")]
    UnexpectedFrame,
    #[error("could not decode a server message: {0}")]
    Decode(String),
    #[error("signaling connection is closed")]
    Closed,
}

/// A live signaling session. Cloneable — every clone shares the same
/// underlying connection (send-side only; each [`connect`] call hands back
/// one receiver for the inbound half, since `SignalMessage`s naturally have
/// exactly one intended reader).
#[derive(Debug, Clone)]
pub struct SignalingClient {
    pub session_id: String,
    /// This device's address as observed by the STUN server (TCP-derived —
    /// see [`swarm_core::signal::SignalMessage::HelloAck`]'s doc comment for
    /// why the UDP reflexive address is a separate lookup).
    pub observed_addr: String,
    pub reflector_ports: Vec<u16>,
    outbound: mpsc::UnboundedSender<SignalMessage>,
}

impl SignalingClient {
    /// Opens the WSS connection, sends `hello`, and waits for `hello_ack`.
    /// The returned receiver carries every subsequent `presence`, `signal`,
    /// and `error` message — `hello`/`hello_ack`/`ping`/`pong` are consumed
    /// internally and never forwarded, so a caller's receive loop only ever
    /// sees messages worth acting on.
    pub async fn connect(
        base_url: &str,
        access_token: &str,
        device_id: &str,
        capabilities: Option<CapabilityProfile>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SignalMessage>), SignalingError> {
        let ws_url = to_ws_url(base_url)?;
        let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| SignalingError::Connect(e.to_string()))?;

        let hello = SignalMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            access_token: access_token.to_string(),
            device_id: device_id.to_string(),
            capabilities,
        };
        ws.send(to_ws_message(&hello))
            .await
            .map_err(|e| SignalingError::Connect(e.to_string()))?;

        let frame = tokio::time::timeout(HELLO_ACK_TIMEOUT, ws.next())
            .await
            .map_err(|_| SignalingError::HelloTimeout)?
            .ok_or(SignalingError::ConnectionClosed)?
            .map_err(|e| SignalingError::Connect(e.to_string()))?;
        let WsMessage::Text(text) = frame else {
            return Err(SignalingError::UnexpectedFrame);
        };
        let message: SignalMessage =
            serde_json::from_str(&text).map_err(|e| SignalingError::Decode(e.to_string()))?;
        let (session_id, observed_addr, reflector_ports) = match message {
            SignalMessage::HelloAck {
                session_id,
                observed_addr,
                reflector_ports,
            } => (session_id, observed_addr, reflector_ports),
            SignalMessage::Error { code, message } => {
                return Err(SignalingError::Rejected { code, message })
            }
            _ => return Err(SignalingError::UnexpectedFrame),
        };

        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        tokio::spawn(run(ws, outbound_rx, inbound_tx));

        Ok((
            Self {
                session_id,
                observed_addr,
                reflector_ports,
                outbound: outbound_tx,
            },
            inbound_rx,
        ))
    }

    /// Queues a message for delivery. Returns [`SignalingError::Closed`] if
    /// the connection has already ended — the caller decides whether that
    /// means reconnecting or giving up, this layer doesn't guess.
    pub fn send(&self, message: SignalMessage) -> Result<(), SignalingError> {
        self.outbound
            .send(message)
            .map_err(|_| SignalingError::Closed)
    }

    /// Convenience for the common case: relay a hole-punch payload to `to`.
    /// `from` is left unset — the server stamps it, and overwrites it if a
    /// caller sets it anyway (see `ws.rs`'s `handle`).
    pub fn send_signal(
        &self,
        to: impl Into<String>,
        payload: swarm_core::signal::SignalPayload,
    ) -> Result<(), SignalingError> {
        self.send(SignalMessage::Signal {
            from: None,
            to: to.into(),
            payload,
        })
    }

    /// Tells the server this session is ending on purpose (vs. a network
    /// drop) — purely a courtesy; the server treats both the same way.
    pub fn bye(&self) -> Result<(), SignalingError> {
        self.send(SignalMessage::Bye {})
    }
}

async fn run(
    mut ws: WsStream,
    mut outbound_rx: mpsc::UnboundedReceiver<SignalMessage>,
    inbound_tx: mpsc::UnboundedSender<SignalMessage>,
) {
    let mut ping_seq: u64 = 0;
    let mut ping_timer = tokio::time::interval(PING_INTERVAL);
    ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping_timer.tick().await; // first tick fires immediately; hello_ack just happened, nothing to prove yet

    loop {
        tokio::select! {
            _ = ping_timer.tick() => {
                ping_seq += 1;
                if ws.send(to_ws_message(&SignalMessage::Ping { seq: ping_seq })).await.is_err() {
                    break;
                }
            }
            outbound = outbound_rx.recv() => {
                match outbound {
                    None => {
                        let _ = ws.send(WsMessage::Close(None)).await;
                        break;
                    }
                    Some(message) => {
                        if ws.send(to_ws_message(&message)).await.is_err() {
                            break;
                        }
                    }
                }
            }
            incoming = ws.next() => {
                match incoming {
                    Some(Ok(WsMessage::Text(text))) => {
                        let Ok(message) = serde_json::from_str::<SignalMessage>(&text) else { continue };
                        match message {
                            // Keepalive/handshake bookkeeping this layer owns — never forwarded.
                            SignalMessage::Pong { .. } | SignalMessage::Hello { .. } | SignalMessage::HelloAck { .. } => {}
                            SignalMessage::Ping { seq } => {
                                if ws.send(to_ws_message(&SignalMessage::Pong { seq })).await.is_err() {
                                    break;
                                }
                            }
                            bye @ SignalMessage::Bye {} => {
                                let _ = inbound_tx.send(bye);
                                break;
                            }
                            other => {
                                if inbound_tx.send(other).is_err() {
                                    break; // caller dropped the receiver — nothing left to do
                                }
                            }
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(_)) => {} // binary/ping/pong at the WS transport level: not this protocol's concern
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

fn to_ws_message(message: &SignalMessage) -> WsMessage {
    WsMessage::Text(serde_json::to_string(message).unwrap_or_default())
}

fn to_ws_url(base_url: &str) -> Result<String, SignalingError> {
    let trimmed = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(SignalingError::InvalidBaseUrl(base_url.to_string()));
    };
    Ok(format!("{ws_base}/api/v1/ws"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_http_and_https_to_ws_urls() {
        assert_eq!(
            to_ws_url("http://example.test:8080").unwrap(),
            "ws://example.test:8080/api/v1/ws"
        );
        assert_eq!(
            to_ws_url("https://example.test/").unwrap(),
            "wss://example.test/api/v1/ws"
        );
    }

    #[test]
    fn rejects_a_base_url_without_a_scheme() {
        assert!(matches!(
            to_ws_url("example.test"),
            Err(SignalingError::InvalidBaseUrl(_))
        ));
    }
}
