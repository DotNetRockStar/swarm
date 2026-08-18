//! End-to-end exercise of the Phase 1 exit criteria, fully in-process:
//! account → swarm → join codes → two device registrations → WSS presence →
//! signal relay → roster fetch → revocation.

use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use stun_server::config::Config;
use stun_server::email::Mailer;
use stun_server::hub::Hub;
use stun_server::routes::build_router;
use stun_server::security::BruteForceBlocker;
use stun_server::state::AppState;
use swarm_core::rest::{DeviceRegistration, DeviceType, RegisterDeviceRequest, RegisterDeviceResponse};
use swarm_core::signal::{Candidate, CandidateKind, SignalMessage, SignalPayload};
use swarm_core::PROTOCOL_VERSION;
use tokio_tungstenite::tungstenite::Message as WsMessage;

async fn spawn_server() -> String {
    let db_path = std::env::temp_dir().join(format!("swarm-e2e-{}.sqlite", stun_server::security::new_id()));
    let db = stun_server::db::connect(db_path.to_str().unwrap()).await.unwrap();
    let config = Config {
        database_path: db_path.display().to_string(),
        http_bind: "127.0.0.1:0".parse().unwrap(),
        reflector_ports: vec![],
        public_url: "http://test.invalid".into(),
        session_ttl_secs: 3600,
        join_code_ttl_secs: 900,
        smtp: None,
    };
    let state =
        Arc::new(AppState { db, hub: Hub::new(), config, blocker: BruteForceBlocker::new(), mailer: Mailer::from_config(None) });
    let router = build_router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    format!("http://{addr}")
}

struct Browser {
    client: reqwest::Client,
    base: String,
    session: String,
    csrf: String,
}

impl Browser {
    async fn login_fresh_account(base: &str, email: &str) -> Self {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("{base}/api/v1/auth/register"))
            .json(&serde_json::json!({"email": email, "password": "correct horse battery"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        let response = client
            .post(format!("{base}/api/v1/auth/login"))
            .json(&serde_json::json!({"email": email, "password": "correct horse battery"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut session = String::new();
        let mut csrf = String::new();
        for value in response.headers().get_all(reqwest::header::SET_COOKIE) {
            let raw = value.to_str().unwrap();
            let (pair, _) = raw.split_once(';').unwrap_or((raw, ""));
            let (name, val) = pair.split_once('=').unwrap();
            match name {
                "swarm_session" => session = val.to_string(),
                "swarm_csrf" => csrf = val.to_string(),
                _ => {}
            }
        }
        assert!(!session.is_empty() && !csrf.is_empty(), "login must set both cookies");
        Self { client, base: base.to_string(), session, csrf }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("cookie", format!("swarm_session={}; swarm_csrf={}", self.session, self.csrf))
            .header("x-swarm-csrf", &self.csrf)
    }
}

type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws_connect(base: &str, device_id: &str, token: &str) -> WsStream {
    let ws_url = base.replace("http://", "ws://") + "/api/v1/ws";
    let (mut stream, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let hello = SignalMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        access_token: token.to_string(),
        device_id: device_id.to_string(),
        capabilities: None,
    };
    stream.send(WsMessage::Text(serde_json::to_string(&hello).unwrap())).await.unwrap();
    let ack = recv_signal(&mut stream).await;
    match ack {
        SignalMessage::HelloAck { observed_addr, .. } => {
            assert!(observed_addr.starts_with("127.0.0.1:"));
        }
        other => panic!("expected hello_ack, got {other:?}"),
    }
    stream
}

async fn recv_signal(stream: &mut WsStream) -> SignalMessage {
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("timed out waiting for signal frame")
            .expect("socket closed")
            .expect("socket error");
        if let WsMessage::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

fn registration(name: &str, device_type: DeviceType, machine: &str, fp_byte: &str) -> DeviceRegistration {
    DeviceRegistration {
        name: name.into(),
        device_type,
        machine_id: machine.into(),
        cert_fingerprint: fp_byte.repeat(32),
        platform: "test".into(),
        app_version: "0.1.0".into(),
        metadata: [("hostname".to_string(), name.to_string())].into(),
    }
}

#[tokio::test]
async fn full_phase1_flow() {
    let base = spawn_server().await;
    let browser = Browser::login_fresh_account(&base, "jerrod@example.com").await;

    // Create a swarm and two single-use join codes.
    let swarm: serde_json::Value = browser
        .request(reqwest::Method::POST, "/api/v1/swarms")
        .json(&serde_json::json!({"name": "Home"}))
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    let swarm_id = swarm["id"].as_str().unwrap().to_string();
    let mut codes = Vec::new();
    for _ in 0..2 {
        let code: serde_json::Value = browser
            .request(reqwest::Method::POST, &format!("/api/v1/swarms/{swarm_id}/codes"))
            .json(&serde_json::json!({}))
            .send().await.unwrap().error_for_status().unwrap()
            .json().await.unwrap();
        codes.push(code["code"].as_str().unwrap().to_string());
    }

    // Register two devices with the codes (no session — pure device flow).
    let anon = reqwest::Client::new();
    let register = |code: String, reg: DeviceRegistration| {
        let anon = anon.clone();
        let base = base.clone();
        async move {
            let response = anon
                .post(format!("{base}/api/v1/devices/register"))
                .json(&RegisterDeviceRequest { code, device: reg })
                .send().await.unwrap();
            assert_eq!(response.status(), 201);
            response.json::<RegisterDeviceResponse>().await.unwrap()
        }
    };
    let media_server = register(codes[0].clone(), registration("Media Server", DeviceType::Server, "m1", "aa")).await;
    let tv_client = register(codes[1].clone(), registration("Living Room TV", DeviceType::Client, "m2", "bb")).await;
    assert_eq!(media_server.swarm.name, "Home");

    // A used code must not redeem twice.
    let reuse = anon
        .post(format!("{base}/api/v1/devices/register"))
        .json(&RegisterDeviceRequest { code: codes[0].clone(), device: registration("Sneaky", DeviceType::Client, "m3", "cc") })
        .send().await.unwrap();
    assert_eq!(reuse.status(), 401);
    let my_devices: serde_json::Value = browser
        .request(reqwest::Method::GET, "/api/v1/me/devices")
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    let registered = my_devices["devices"].as_array().unwrap();
    assert_eq!(registered.len(), 2);
    assert!(registered.iter().all(|device| device["name"] != "Sneaky"),
        "failed registration must roll its device row back");

    // Server device connects to signaling; then the TV connects and the
    // server should be told the TV came online.
    let mut server_ws = ws_connect(&base, &media_server.device_id, &media_server.access_token).await;
    let mut tv_ws = ws_connect(&base, &tv_client.device_id, &tv_client.access_token).await;
    match recv_signal(&mut server_ws).await {
        SignalMessage::Presence { device_id, online, device_type, .. } => {
            assert_eq!(device_id, tv_client.device_id);
            assert!(online);
            assert_eq!(device_type, DeviceType::Client);
        }
        other => panic!("expected presence, got {other:?}"),
    }

    // TV sends a hole-punch offer to the server; it arrives with `from` stamped.
    let offer = SignalMessage::Signal {
        from: None,
        to: media_server.device_id.clone(),
        payload: SignalPayload::Offer {
            punch_id: "p1".into(),
            candidates: vec![Candidate { kind: CandidateKind::Lan, ip: "192.168.1.20".into(), port: 40001 }],
            cert_fingerprint: "bb".repeat(32),
        },
    };
    tv_ws.send(WsMessage::Text(serde_json::to_string(&offer).unwrap())).await.unwrap();
    match recv_signal(&mut server_ws).await {
        SignalMessage::Signal { from, payload: SignalPayload::Offer { punch_id, .. }, .. } => {
            assert_eq!(from.as_deref(), Some(tv_client.device_id.as_str()));
            assert_eq!(punch_id, "p1");
        }
        other => panic!("expected relayed offer, got {other:?}"),
    }

    // Device-side roster fetch (Bearer): both devices, TV online, fingerprints pinned.
    let roster: swarm_core::rest::SwarmDevicesResponse = anon
        .get(format!("{base}/api/v1/swarms/{swarm_id}/devices"))
        .bearer_auth(&media_server.access_token)
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    assert_eq!(roster.devices.len(), 2);
    let tv_row = roster.devices.iter().find(|d| d.device_id == tv_client.device_id).unwrap();
    assert!(tv_row.online);
    assert_eq!(tv_row.cert_fingerprint, "bb".repeat(32));
    assert_eq!(tv_row.metadata.get("hostname").map(String::as_str), Some("Living Room TV"));

    // Revoke the TV from the browser; its token dies and the server hears it went offline.
    browser
        .request(reqwest::Method::DELETE, &format!("/api/v1/devices/{}", tv_client.device_id))
        .send().await.unwrap().error_for_status().unwrap();
    let denied = anon
        .get(format!("{base}/api/v1/swarms/{swarm_id}/devices"))
        .bearer_auth(&tv_client.access_token)
        .send().await.unwrap();
    assert_eq!(denied.status(), 401);
    match recv_signal(&mut tv_ws).await {
        SignalMessage::Error { code, .. } => assert_eq!(code, "revoked"),
        other => panic!("expected revoked error, got {other:?}"),
    }
}

#[tokio::test]
async fn leave_swarm_removes_only_that_membership() {
    let base = spawn_server().await;
    let browser = Browser::login_fresh_account(&base, "multi-swarm@example.com").await;
    let anon = reqwest::Client::new();

    // Two swarms owned by the same account, one join code each.
    let mut swarm_ids = Vec::new();
    let mut codes = Vec::new();
    for name in ["Home", "Cabin"] {
        let swarm: serde_json::Value = browser
            .request(reqwest::Method::POST, "/api/v1/swarms")
            .json(&serde_json::json!({"name": name}))
            .send().await.unwrap().error_for_status().unwrap()
            .json().await.unwrap();
        swarm_ids.push(swarm["id"].as_str().unwrap().to_string());
        let code: serde_json::Value = browser
            .request(reqwest::Method::POST, &format!("/api/v1/swarms/{}/codes", swarm_ids.last().unwrap()))
            .json(&serde_json::json!({}))
            .send().await.unwrap().error_for_status().unwrap()
            .json().await.unwrap();
        codes.push(code["code"].as_str().unwrap().to_string());
    }

    // Register into the first swarm, then join the second with the device's own token.
    let registered: RegisterDeviceResponse = anon
        .post(format!("{base}/api/v1/devices/register"))
        .json(&RegisterDeviceRequest { code: codes[0].clone(), device: registration("Roamer", DeviceType::Both, "roam1", "dd") })
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    anon.post(format!("{base}/api/v1/swarms/join"))
        .bearer_auth(&registered.access_token)
        .json(&serde_json::json!({"code": codes[1]}))
        .send().await.unwrap().error_for_status().unwrap();

    let devices_before: serde_json::Value = browser
        .request(reqwest::Method::GET, "/api/v1/me/devices")
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    let swarms_before = devices_before["devices"][0]["swarms"].as_array().unwrap();
    assert_eq!(swarms_before.len(), 2, "device should be in both swarms before leaving either");

    // Leave the first swarm only.
    let left = anon
        .delete(format!("{base}/api/v1/swarms/{}/devices/{}", swarm_ids[0], registered.device_id))
        .bearer_auth(&registered.access_token)
        .send().await.unwrap();
    assert_eq!(left.status(), 200);

    let devices_after: serde_json::Value = browser
        .request(reqwest::Method::GET, "/api/v1/me/devices")
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    let swarms_after = devices_after["devices"][0]["swarms"].as_array().unwrap();
    assert_eq!(swarms_after.len(), 1, "leaving one swarm must not touch the other membership");
    assert_eq!(swarms_after[0]["id"].as_str().unwrap(), swarm_ids[1]);

    // Leaving again is a harmless no-op, not an error.
    let repeat = anon
        .delete(format!("{base}/api/v1/swarms/{}/devices/{}", swarm_ids[0], registered.device_id))
        .bearer_auth(&registered.access_token)
        .send().await.unwrap();
    assert_eq!(repeat.status(), 200);

    // The device can no longer see the roster of a swarm it left.
    let denied = anon
        .get(format!("{base}/api/v1/swarms/{}/devices", swarm_ids[0]))
        .bearer_auth(&registered.access_token)
        .send().await.unwrap();
    assert_eq!(denied.status(), 403);
}

#[tokio::test]
async fn leave_swarm_is_self_only() {
    let base = spawn_server().await;
    let browser = Browser::login_fresh_account(&base, "self-only@example.com").await;
    let anon = reqwest::Client::new();

    let swarm: serde_json::Value = browser
        .request(reqwest::Method::POST, "/api/v1/swarms")
        .json(&serde_json::json!({"name": "Shared"}))
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    let swarm_id = swarm["id"].as_str().unwrap().to_string();
    let mut codes = Vec::new();
    for _ in 0..2 {
        let code: serde_json::Value = browser
            .request(reqwest::Method::POST, &format!("/api/v1/swarms/{swarm_id}/codes"))
            .json(&serde_json::json!({}))
            .send().await.unwrap().error_for_status().unwrap()
            .json().await.unwrap();
        codes.push(code["code"].as_str().unwrap().to_string());
    }

    let device_a: RegisterDeviceResponse = anon
        .post(format!("{base}/api/v1/devices/register"))
        .json(&RegisterDeviceRequest { code: codes[0].clone(), device: registration("A", DeviceType::Client, "a1", "ee") })
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    let device_b: RegisterDeviceResponse = anon
        .post(format!("{base}/api/v1/devices/register"))
        .json(&RegisterDeviceRequest { code: codes[1].clone(), device: registration("B", DeviceType::Client, "b1", "ff") })
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();

    // A tries to remove B's membership using A's own token — forbidden.
    let denied = anon
        .delete(format!("{base}/api/v1/swarms/{swarm_id}/devices/{}", device_b.device_id))
        .bearer_auth(&device_a.access_token)
        .send().await.unwrap();
    assert_eq!(denied.status(), 403);

    // B is still in the roster.
    let roster: swarm_core::rest::SwarmDevicesResponse = anon
        .get(format!("{base}/api/v1/swarms/{swarm_id}/devices"))
        .bearer_auth(&device_b.access_token)
        .send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    assert!(roster.devices.iter().any(|d| d.device_id == device_b.device_id));
}

#[tokio::test]
async fn ws_rejects_bad_token_and_wrong_protocol() {
    let base = spawn_server().await;
    let ws_url = base.replace("http://", "ws://") + "/api/v1/ws";

    let (mut stream, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let hello = SignalMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        access_token: "bogus".into(),
        device_id: "nobody".into(),
        capabilities: None,
    };
    stream.send(WsMessage::Text(serde_json::to_string(&hello).unwrap())).await.unwrap();
    match recv_signal(&mut stream).await {
        SignalMessage::Error { code, .. } => assert_eq!(code, "unauthorized"),
        other => panic!("expected unauthorized, got {other:?}"),
    }

    let (mut stream, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let hello = SignalMessage::Hello {
        protocol_version: PROTOCOL_VERSION + 1,
        access_token: "x".into(),
        device_id: "y".into(),
        capabilities: None,
    };
    stream.send(WsMessage::Text(serde_json::to_string(&hello).unwrap())).await.unwrap();
    match recv_signal(&mut stream).await {
        SignalMessage::Error { code, .. } => assert_eq!(code, "protocol_version"),
        other => panic!("expected protocol_version error, got {other:?}"),
    }
}

#[tokio::test]
async fn csrf_and_auth_gates() {
    let base = spawn_server().await;
    let browser = Browser::login_fresh_account(&base, "gates@example.com").await;

    // Mutation without the CSRF header is refused.
    let no_csrf = browser
        .client
        .post(format!("{base}/api/v1/swarms"))
        .header("cookie", format!("swarm_session={}; swarm_csrf={}", browser.session, browser.csrf))
        .json(&serde_json::json!({"name": "Nope"}))
        .send().await.unwrap();
    assert_eq!(no_csrf.status(), 403);

    // Anonymous swarm listing is refused.
    let anon = reqwest::Client::new();
    let denied = anon.get(format!("{base}/api/v1/swarms")).send().await.unwrap();
    assert_eq!(denied.status(), 401);

    // Weak password is refused at registration.
    let weak = anon
        .post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({"email": "weak@example.com", "password": "short"}))
        .send().await.unwrap();
    assert_eq!(weak.status(), 400);
}
