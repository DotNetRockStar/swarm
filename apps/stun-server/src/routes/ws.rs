//! WSS signaling endpoint (`/api/v1/ws`).
//!
//! First frame must be `hello`; the server validates the access token against
//! the claimed device, replies `hello_ack`, and joins the device to the hub.
//! `signal` frames are routed only between devices sharing a swarm, with
//! `from` stamped server-side; payloads are never interpreted. Presence
//! deltas fan out to online swarm-mates on connect, disconnect, and when a
//! device pushes an updated `presence` (e.g. a server advertising transcode
//! load).

use crate::db::now;
use crate::routes::swarms::parse_device_type;
use crate::security::{new_id, token_hash};
use crate::state::SharedState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::IntoResponse;
use std::net::SocketAddr;
use swarm_core::signal::{SignalMessage, StreamingStatus};
use swarm_core::PROTOCOL_VERSION;

pub async fn upgrade(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle(state, addr, socket))
}

fn to_frame(message: &SignalMessage) -> Message {
    Message::Text(serde_json::to_string(message).unwrap_or_default().into())
}

async fn send(socket: &mut WebSocket, message: &SignalMessage) -> bool {
    socket.send(to_frame(message)).await.is_ok()
}

/// Online swarm-mates of `device_id`, each with the swarm ids they share.
async fn swarm_mates(state: &SharedState, device_id: &str) -> Vec<(String, Vec<String>)> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT sd2.device_id, GROUP_CONCAT(sd1.swarm_id) FROM swarm_devices sd1 \
         JOIN swarm_devices sd2 ON sd2.swarm_id = sd1.swarm_id AND sd2.device_id != sd1.device_id \
         WHERE sd1.device_id = ? GROUP BY sd2.device_id",
    )
    .bind(device_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(mate, swarms)| (mate, swarms.split(',').map(str::to_string).collect()))
        .collect()
}

async fn shares_swarm(state: &SharedState, a: &str, b: &str) -> bool {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM swarm_devices sd1 JOIN swarm_devices sd2 ON sd2.swarm_id = sd1.swarm_id \
         WHERE sd1.device_id = ? AND sd2.device_id = ? LIMIT 1",
    )
    .bind(a)
    .bind(b)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    row.is_some()
}

async fn broadcast_presence(
    state: &SharedState,
    device_id: &str,
    device_type: swarm_core::rest::DeviceType,
    online: bool,
    streaming: Option<StreamingStatus>,
) {
    for (mate, shared_swarm_ids) in swarm_mates(state, device_id).await {
        state.hub.send_to(
            &mate,
            SignalMessage::Presence {
                device_id: device_id.to_string(),
                device_type,
                online,
                swarm_ids: shared_swarm_ids,
                streaming: streaming.clone(),
            },
        );
    }
}

async fn handle(state: SharedState, addr: SocketAddr, mut socket: WebSocket) {
    // --- Handshake: first frame must be a valid hello within 10s. ---
    let hello = tokio::time::timeout(std::time::Duration::from_secs(10), socket.recv()).await;
    let Ok(Some(Ok(Message::Text(text)))) = hello else {
        return;
    };
    let parsed: Result<SignalMessage, _> = serde_json::from_str(&text);
    let (device_id, token) = match parsed {
        Ok(SignalMessage::Hello {
            protocol_version,
            access_token,
            device_id,
            ..
        }) => {
            if protocol_version != PROTOCOL_VERSION {
                let _ = send(
                    &mut socket,
                    &SignalMessage::Error {
                        code: "protocol_version".into(),
                        message: format!(
                            "server speaks protocol v{PROTOCOL_VERSION}; please update"
                        ),
                    },
                )
                .await;
                return;
            }
            (device_id, access_token)
        }
        _ => {
            let _ = send(
                &mut socket,
                &SignalMessage::Error {
                    code: "expected_hello".into(),
                    message: "first frame must be hello".into(),
                },
            )
            .await;
            return;
        }
    };
    let row: Option<(String, Option<i64>)> = sqlx::query_as(
        "SELECT device_type, revoked_at FROM devices WHERE id = ? AND access_token_hash = ?",
    )
    .bind(&device_id)
    .bind(token_hash(&token))
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let Some((device_type_raw, None)) = row else {
        state.blocker.record_failure(addr.ip());
        let _ = send(
            &mut socket,
            &SignalMessage::Error {
                code: "unauthorized".into(),
                message: "unknown device or bad token".into(),
            },
        )
        .await;
        return;
    };
    let device_type = parse_device_type(&device_type_raw);

    let session_id = new_id();
    let mut inbox = state.hub.connect(&device_id, &session_id);
    if !send(
        &mut socket,
        &SignalMessage::HelloAck {
            session_id: session_id.clone(),
            observed_addr: addr.to_string(),
            reflector_ports: state.config.reflector_ports.clone(),
        },
    )
    .await
    {
        state.hub.disconnect(&device_id, &session_id);
        return;
    }
    let _ = sqlx::query("UPDATE devices SET last_seen_at = ? WHERE id = ?")
        .bind(now())
        .bind(&device_id)
        .execute(&state.db)
        .await;
    tracing::info!(device_id, %addr, "signaling session open");
    broadcast_presence(&state, &device_id, device_type, true, None).await;

    // --- Main loop: fan hub messages out, route socket messages in. ---
    loop {
        tokio::select! {
            outbound = inbox.recv() => {
                match outbound {
                    // Channel closed means a newer connection replaced us.
                    None => break,
                    Some(message) => {
                        if !send(&mut socket, &message).await {
                            break;
                        }
                    }
                }
            }
            inbound = socket.recv() => {
                let Some(Ok(frame)) = inbound else { break };
                let Message::Text(text) = frame else { continue };
                let Ok(message) = serde_json::from_str::<SignalMessage>(&text) else {
                    let _ = send(&mut socket, &SignalMessage::Error {
                        code: "bad_message".into(),
                        message: "unparseable signal message".into(),
                    }).await;
                    continue;
                };
                match message {
                    SignalMessage::Ping { seq } => {
                        if !send(&mut socket, &SignalMessage::Pong { seq }).await {
                            break;
                        }
                    }
                    SignalMessage::Signal { to, payload, .. } => {
                        if !shares_swarm(&state, &device_id, &to).await {
                            let _ = send(&mut socket, &SignalMessage::Error {
                                code: "not_swarm_mates".into(),
                                message: "target device does not share a swarm with you".into(),
                            }).await;
                            continue;
                        }
                        let delivered = state.hub.send_to(&to, SignalMessage::Signal {
                            from: Some(device_id.clone()),
                            to: to.clone(),
                            payload,
                        });
                        if !delivered {
                            let _ = send(&mut socket, &SignalMessage::Error {
                                code: "peer_offline".into(),
                                message: format!("device {to} has no live signaling session"),
                            }).await;
                        }
                    }
                    // A device may push presence to update its streaming
                    // status; identity fields are overridden server-side.
                    SignalMessage::Presence { streaming, .. } => {
                        broadcast_presence(&state, &device_id, device_type, true, streaming).await;
                    }
                    SignalMessage::Bye {} => break,
                    // hello twice, acks, pongs, errors from clients: ignore.
                    _ => {}
                }
            }
        }
    }

    state.hub.disconnect(&device_id, &session_id);
    let _ = sqlx::query("UPDATE devices SET last_seen_at = ? WHERE id = ?")
        .bind(now())
        .bind(&device_id)
        .execute(&state.db)
        .await;
    tracing::info!(device_id, "signaling session closed");
    broadcast_presence(&state, &device_id, device_type, false, None).await;
}
