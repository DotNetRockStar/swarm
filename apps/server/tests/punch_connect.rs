//! Proves the full connection-establishment flow end to end: two real
//! devices, registered into the same swarm on a real (in-process) STUN
//! server, each holding a real `SignalingClient` session and a real
//! reflector to query, negotiate a hole punch purely over signaling
//! (`Offer` -> `Answer` -> punch -> mutual `Punched`), and end up with a
//! real pinned QUIC connection that carries an actual request/response.
//! Everything below this test (signaling, reflector, punch, the punch-to-
//! QUIC socket handoff) already has its own focused unit test; this is the
//! thing none of those can show alone — that `punch_connect` wires them
//! together correctly.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use stun_server::config::Config as StunConfig;
use stun_server::email::Mailer;
use stun_server::hub::Hub;
use stun_server::routes::build_router;
use stun_server::security::BruteForceBlocker;
use stun_server::state::AppState;
use swarm_core::peer::{PeerRequest, PeerResponseHeader};
use swarm_core::rest::{DeviceRegistration, DeviceType};
use swarm_core::signal::{SignalMessage, SignalPayload};
use swarm_p2p::endpoint::{read_body, read_request, send_request, write_response_header};
use swarm_p2p::identity::ensure_identity;
use swarm_p2p::pin::AllowedPeers;
use swarm_server::punch_connect::{
    initiate_punch_connection, respond_to_punch_offer, ReceivedOffer,
};
use swarm_stun_client::{SignalingClient, StunClient};

async fn spawn_stun_server() -> String {
    let db_path = std::env::temp_dir().join(format!(
        "swarm-punch-connect-stun-{}.sqlite",
        stun_server::security::new_id()
    ));
    let db = stun_server::db::connect(db_path.to_str().unwrap())
        .await
        .unwrap();
    let config = StunConfig {
        database_path: db_path.display().to_string(),
        http_bind: "127.0.0.1:0".parse().unwrap(),
        reflector_ports: vec![],
        public_url: "http://test.invalid".into(),
        session_ttl_secs: 3600,
        join_code_ttl_secs: 900,
        activation_ttl_secs: 600,
        managed_swarm_lease_secs: 2_592_000,
        managed_swarm_max_clients: 20,
        smtp: None,
    };
    let state = Arc::new(AppState {
        db,
        hub: Hub::new(),
        config,
        blocker: BruteForceBlocker::new(),
        activation_allocations: stun_server::security::AllocationLimiter::new(
            20,
            std::time::Duration::from_secs(3600),
        ),
        managed_swarm_allocations: stun_server::security::AllocationLimiter::new(
            5,
            std::time::Duration::from_secs(3600),
        ),
        mailer: Mailer::from_config(None),
    });
    let router = build_router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
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
        let password = "correct horse battery";
        client
            .post(format!("{base}/api/v1/auth/register"))
            .json(&serde_json::json!({"email": email, "password": password}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let response = client
            .post(format!("{base}/api/v1/auth/login"))
            .json(&serde_json::json!({"email": email, "password": password}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
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
        Self {
            client,
            base: base.to_string(),
            session,
            csrf,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header(
                "cookie",
                format!("swarm_session={}; swarm_csrf={}", self.session, self.csrf),
            )
            .header("x-swarm-csrf", &self.csrf)
    }

    async fn create_swarm(&self, name: &str) -> String {
        let body: serde_json::Value = self
            .request(reqwest::Method::POST, "/api/v1/swarms")
            .json(&serde_json::json!({"name": name}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        body["id"].as_str().unwrap().to_string()
    }

    async fn create_code(&self, swarm_id: &str) -> String {
        let body: serde_json::Value = self
            .request(
                reqwest::Method::POST,
                &format!("/api/v1/swarms/{swarm_id}/codes"),
            )
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        body["code"].as_str().unwrap().to_string()
    }
}

async fn spawn_reflector() -> SocketAddr {
    let probe = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    tokio::spawn(stun_server::reflector::run(port));
    tokio::time::sleep(Duration::from_millis(50)).await;
    format!("127.0.0.1:{port}").parse().unwrap()
}

#[tokio::test]
async fn full_offer_answer_punch_confirm_and_quic_connect() {
    let stun_base = spawn_stun_server().await;
    let reflector_addr = spawn_reflector().await;
    let base = std::env::temp_dir().join(format!("swarm-punch-connect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);

    let browser = Browser::login_fresh_account(&stun_base, "punch-connect@example.com").await;
    let swarm_id = browser.create_swarm("Home").await;

    let a_identity = ensure_identity(&base.join("a-id")).unwrap();
    let b_identity = ensure_identity(&base.join("b-id")).unwrap();

    let a_code = browser.create_code(&swarm_id).await;
    let a_registration = DeviceRegistration {
        name: "A".into(),
        device_type: DeviceType::Both,
        machine_id: "machine-a".into(),
        cert_fingerprint: a_identity.fingerprint.clone(),
        platform: "test".into(),
        app_version: "0.1.0".into(),
        metadata: Default::default(),
    };
    let a_reg = StunClient::new(&stun_base)
        .register_device(&a_code, a_registration)
        .await
        .unwrap();

    let b_code = browser.create_code(&swarm_id).await;
    let b_registration = DeviceRegistration {
        name: "B".into(),
        device_type: DeviceType::Both,
        machine_id: "machine-b".into(),
        cert_fingerprint: b_identity.fingerprint.clone(),
        platform: "test".into(),
        app_version: "0.1.0".into(),
        metadata: Default::default(),
    };
    let b_reg = StunClient::new(&stun_base)
        .register_device(&b_code, b_registration)
        .await
        .unwrap();

    let (a_signaling, mut a_rx) =
        SignalingClient::connect(&stun_base, &a_reg.access_token, &a_reg.device_id, None)
            .await
            .unwrap();
    let (b_signaling, mut b_rx) =
        SignalingClient::connect(&stun_base, &b_reg.access_token, &b_reg.device_id, None)
            .await
            .unwrap();

    let b_allowed = AllowedPeers::new();
    b_allowed.replace([a_identity.fingerprint.clone()]);

    let initiator = initiate_punch_connection(
        &a_signaling,
        &mut a_rx,
        reflector_addr,
        &b_reg.device_id,
        &a_identity,
        &b_identity.fingerprint,
    );
    let responder = async {
        let offer = loop {
            match b_rx.recv().await.expect("b's signaling channel closed") {
                SignalMessage::Signal {
                    from: Some(from),
                    payload:
                        SignalPayload::Offer {
                            punch_id,
                            candidates,
                            cert_fingerprint,
                        },
                    ..
                } => {
                    break ReceivedOffer {
                        from,
                        punch_id,
                        candidates,
                        cert_fingerprint,
                    };
                }
                _ => continue,
            }
        };
        respond_to_punch_offer(
            &b_signaling,
            &mut b_rx,
            reflector_addr,
            offer,
            &b_identity,
            b_allowed,
        )
        .await
    };

    let (a_connection, b_connection) = tokio::join!(initiator, responder);
    let a_connection = a_connection.unwrap();
    let b_connection = b_connection.unwrap();

    // The connection is real, not just "handshake completed" — prove a
    // request/response actually flows, server side driven by B.
    let accept_task = tokio::spawn(async move {
        let (mut send, mut recv) = b_connection.accept_bi().await.unwrap();
        let request = read_request(&mut recv).await.unwrap();
        assert_eq!(request.path, "/ping");
        let header = PeerResponseHeader {
            status: 200,
            len: 4,
            content_type: None,
            content_range: None,
            etag: None,
        };
        write_response_header(&mut send, &header).await.unwrap();
        send.write_all(b"pong").await.unwrap();
        send.finish().ok();
        // Don't drop `b_connection` until A explicitly closes (see
        // punch_to_quic.rs's doc comment) — otherwise A's read can race a
        // teardown that happens before the last bytes are acked.
        let _ = b_connection.closed().await;
    });

    let request = PeerRequest {
        path: "/ping".into(),
        range: None,
        if_none_match: None,
        playback: None,
        error_report: None,
        like: None,
    };
    let (header, mut recv) = send_request(&a_connection, &request).await.unwrap();
    assert_eq!(header.status, 200);
    let body = read_body(&header, &mut recv).await.unwrap();
    assert_eq!(&body, b"pong");

    a_connection.close(0u32.into(), b"done");
    accept_task.await.unwrap();
}
