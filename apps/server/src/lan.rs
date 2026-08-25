//! SWARM-free LAN discovery and TV-first activation.
//!
//! The media server advertises its QUIC endpoint and certificate fingerprint
//! over mDNS. Discovery is intentionally not authorization. A new Android TV
//! asks this server for a short-lived activation code, displays that code, and
//! privately polls with an unrelated random token. The user enters the visible
//! code in the media-server UI; approval persists the TV certificate in
//! `server-state.sqlite` and adds it to the same `AllowedPeers` set used by
//! SWARM roster members. All catalog and playback traffic still uses mTLS.

use crate::state_db::StateDb;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use swarm_p2p::pin::AllowedPeers;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

pub const SERVICE_TYPE: &str = "_swarm-peer._udp.local.";
const ACTIVATION_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING_ACTIVATIONS: usize = 32;
const MAX_PAIR_REQUEST: usize = 4096;

#[derive(Debug, Clone, Serialize)]
pub struct LanPairingApproval {
    pub name: String,
    pub fingerprint: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LanPairingError {
    #[error("No pending LAN TV uses that code, or the code has expired.")]
    InvalidCode,
    #[error("Could not save the LAN TV approval: {0}")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
struct PendingActivation {
    activation_id: String,
    poll_token: String,
    code: String,
    name: String,
    fingerprint: String,
    requester_ip: IpAddr,
    expires_at: Instant,
    approved: bool,
}

#[derive(Default)]
struct PairingState {
    activations: Vec<PendingActivation>,
}

#[derive(Debug)]
struct ActivationStarted {
    activation_id: String,
    poll_token: String,
    code: String,
    expires_in_seconds: u64,
}

impl PairingState {
    fn purge_expired(&mut self) {
        let now = Instant::now();
        self.activations
            .retain(|activation| activation.expires_at > now);
    }

    fn begin(
        &mut self,
        name: String,
        fingerprint: String,
        requester_ip: IpAddr,
    ) -> Result<ActivationStarted, &'static str> {
        self.purge_expired();
        if let Some(existing) = self.activations.iter().find(|activation| {
            activation.fingerprint == fingerprint && activation.requester_ip == requester_ip
        }) {
            return Ok(started_from(existing));
        }
        if self.activations.len() >= MAX_PENDING_ACTIVATIONS {
            return Err("too_many_pending_activations");
        }

        let code = loop {
            let candidate = format!("{:08}", rand::random::<u32>() % 100_000_000);
            if self
                .activations
                .iter()
                .all(|activation| activation.code != candidate)
            {
                break candidate;
            }
        };
        let activation = PendingActivation {
            activation_id: hex::encode(rand::random::<[u8; 16]>()),
            poll_token: hex::encode(rand::random::<[u8; 24]>()),
            code,
            name,
            fingerprint,
            requester_ip,
            expires_at: Instant::now() + ACTIVATION_TTL,
            approved: false,
        };
        let started = started_from(&activation);
        self.activations.push(activation);
        Ok(started)
    }

    fn poll(
        &mut self,
        activation_id: &str,
        poll_token: &str,
        requester_ip: IpAddr,
    ) -> &'static str {
        self.purge_expired();
        self.activations
            .iter()
            .find(|activation| {
                activation.activation_id == activation_id
                    && activation.poll_token == poll_token
                    && activation.requester_ip == requester_ip
            })
            .map(|activation| {
                if activation.approved {
                    "approved"
                } else {
                    "pending"
                }
            })
            .unwrap_or("expired")
    }
}

fn started_from(activation: &PendingActivation) -> ActivationStarted {
    ActivationStarted {
        activation_id: activation.activation_id.clone(),
        poll_token: activation.poll_token.clone(),
        code: activation.code.clone(),
        expires_in_seconds: activation
            .expires_at
            .saturating_duration_since(Instant::now())
            .as_secs()
            .max(1),
    }
}

struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

pub struct LanService {
    pairing: Arc<Mutex<PairingState>>,
    state_db: Arc<StateDb>,
    allowed: AllowedPeers,
    advertisement: Option<Advertisement>,
}

impl LanService {
    pub async fn start(
        server_fingerprint: String,
        peer_addr: SocketAddr,
        allowed: AllowedPeers,
        state_db: Arc<StateDb>,
        http_media_port: u16,
    ) -> std::io::Result<Self> {
        // TCP and QUIC/UDP can share the same numeric port. Keeping activation
        // on the peer port avoids an unpredictable firewall exception.
        let listener = match TcpListener::bind((Ipv4Addr::UNSPECIFIED, peer_addr.port())).await {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await?
            }
            Err(error) => return Err(error),
        };
        let pairing_port = listener.local_addr()?.port();
        let pairing = Arc::new(Mutex::new(PairingState::default()));
        let listener_pairing = Arc::clone(&pairing);
        tokio::spawn(async move {
            loop {
                let (socket, remote) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(err) => {
                        tracing::warn!(%err, "LAN activation listener stopped");
                        return;
                    }
                };
                let state = Arc::clone(&listener_pairing);
                tokio::spawn(async move {
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        handle_pair_request(socket, remote, state),
                    )
                    .await
                    {
                        Ok(Err(err)) => {
                            tracing::debug!(%remote, %err, "LAN activation request failed")
                        }
                        Err(_) => tracing::debug!(%remote, "LAN activation request timed out"),
                        Ok(Ok(())) => {}
                    }
                });
            }
        });

        let advertisement = advertise(&server_fingerprint, peer_addr, pairing_port, http_media_port);
        Ok(Self {
            pairing,
            state_db,
            allowed,
            advertisement,
        })
    }

    pub async fn approve_pairing_code(
        &self,
        code: &str,
    ) -> Result<LanPairingApproval, LanPairingError> {
        approve_pending_pairing(&self.pairing, &self.state_db, &self.allowed, code).await
    }
}

impl Drop for LanService {
    fn drop(&mut self) {
        if let Some(advertisement) = &self.advertisement {
            let _ = advertisement.daemon.unregister(&advertisement.fullname);
            let _ = advertisement.daemon.shutdown();
        }
    }
}

fn advertise(
    server_fingerprint: &str,
    peer_addr: SocketAddr,
    pairing_port: u16,
    http_media_port: u16,
) -> Option<Advertisement> {
    let daemon = ServiceDaemon::new()
        .map_err(|err| tracing::warn!(%err, "could not start mDNS advertiser"))
        .ok()?;
    let short = &server_fingerprint[..server_fingerprint.len().min(12)];
    let instance = format!("SWARM Media Server {short}");
    let hostname = format!("swarm-{short}.local.");
    let peer_port = peer_addr.port().to_string();
    let pair_port = pairing_port.to_string();
    // Not consumed by any existing client — the Fire TV client only ever
    // reads fingerprint/peer_port/pair_port here and pairs over QUIC, never
    // this port. Advertised for a future HTTP-only client (Roku) that can't
    // speak QUIC at all and has no other way to discover this port; adding
    // it now costs nothing and means that client's own resolver work
    // doesn't also need a server-side change.
    let http_media_port_str = http_media_port.to_string();
    let properties = [
        ("protocol", "2"),
        ("name", "SWARM Media Server"),
        ("fingerprint", server_fingerprint),
        ("peer_port", peer_port.as_str()),
        ("pair_port", pair_port.as_str()),
        ("http_media_port", http_media_port_str.as_str()),
    ];
    let address = swarm_p2p::local_addr::detect_local_ipv4().to_string();
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &hostname,
        address,
        peer_addr.port(),
        &properties[..],
    )
    .map_err(|err| tracing::warn!(%err, "could not build mDNS advertisement"))
    .ok()?;
    let fullname = info.get_fullname().to_string();
    daemon
        .register(info)
        .map_err(|err| tracing::warn!(%err, "could not register mDNS advertisement"))
        .ok()?;
    tracing::info!(
        service = %fullname,
        pairing_port,
        http_media_port,
        "advertising media server on the LAN"
    );
    Some(Advertisement { daemon, fullname })
}

#[derive(Deserialize)]
struct PairRequest {
    action: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    fingerprint: Option<String>,
    #[serde(default)]
    activation_id: Option<String>,
    #[serde(default)]
    poll_token: Option<String>,
}

#[derive(Default, Serialize)]
struct PairResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    poll_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'static str>,
}

async fn handle_pair_request(
    mut socket: TcpStream,
    remote: SocketAddr,
    pairing: Arc<Mutex<PairingState>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !is_lan_address(remote.ip()) {
        write_response(
            &mut socket,
            PairResponse {
                error: Some("not_lan"),
                ..PairResponse::default()
            },
        )
        .await?;
        return Ok(());
    }
    let request_bytes = read_request(&mut socket).await?;
    let request: PairRequest = match serde_json::from_slice(&request_bytes) {
        Ok(request) => request,
        Err(_) => {
            write_response(
                &mut socket,
                PairResponse {
                    error: Some("invalid_request"),
                    ..PairResponse::default()
                },
            )
            .await?;
            return Ok(());
        }
    };

    match request.action.as_str() {
        "begin" => {
            let fingerprint = request
                .fingerprint
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return reject(&mut socket, "invalid_fingerprint").await;
            }
            let name = request.name.unwrap_or_default().trim().to_string();
            if name.is_empty() || name.len() > 80 {
                return reject(&mut socket, "invalid_name").await;
            }
            let started =
                pairing
                    .lock()
                    .await
                    .begin(name.clone(), fingerprint.clone(), remote.ip());
            match started {
                Ok(started) => {
                    tracing::info!(client = %name, fingerprint = %fingerprint, %remote, "created pending LAN TV activation");
                    write_response(
                        &mut socket,
                        PairResponse {
                            ok: true,
                            code: Some(started.code),
                            activation_id: Some(started.activation_id),
                            poll_token: Some(started.poll_token),
                            expires_in_seconds: Some(started.expires_in_seconds),
                            status: Some("pending"),
                            ..PairResponse::default()
                        },
                    )
                    .await?;
                }
                Err(error) => reject(&mut socket, error).await?,
            }
        }
        "poll" => {
            let activation_id = request.activation_id.unwrap_or_default();
            let poll_token = request.poll_token.unwrap_or_default();
            if activation_id.is_empty() || poll_token.is_empty() {
                return reject(&mut socket, "invalid_request").await;
            }
            let status = pairing
                .lock()
                .await
                .poll(&activation_id, &poll_token, remote.ip());
            write_response(
                &mut socket,
                PairResponse {
                    ok: true,
                    status: Some(status),
                    ..PairResponse::default()
                },
            )
            .await?;
        }
        _ => reject(&mut socket, "invalid_request").await?,
    }
    Ok(())
}

async fn approve_pending_pairing(
    pairing: &Arc<Mutex<PairingState>>,
    state_db: &Arc<StateDb>,
    allowed: &AllowedPeers,
    code: &str,
) -> Result<LanPairingApproval, LanPairingError> {
    let normalized_code: String = code.chars().filter(char::is_ascii_digit).collect();
    let mut state = pairing.lock().await;
    state.purge_expired();
    let activation = state
        .activations
        .iter_mut()
        .find(|activation| activation.code == normalized_code)
        .ok_or(LanPairingError::InvalidCode)?;
    state_db
        .save_local_peer(&activation.fingerprint, &activation.name)
        .await?;
    allowed.insert(&activation.fingerprint);
    activation.approved = true;
    let approval = LanPairingApproval {
        name: activation.name.clone(),
        fingerprint: activation.fingerprint.clone(),
    };
    tracing::info!(client = %approval.name, fingerprint = %approval.fingerprint, "approved local TV activation");
    Ok(approval)
}

async fn read_request(socket: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut request_bytes = Vec::new();
    loop {
        if request_bytes.len() >= MAX_PAIR_REQUEST {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request too large",
            ));
        }
        let mut byte = [0u8; 1];
        if socket.read(&mut byte).await? == 0 || byte[0] == b'\n' {
            break;
        }
        request_bytes.push(byte[0]);
    }
    Ok(request_bytes)
}

async fn reject(
    socket: &mut TcpStream,
    error: &'static str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    write_response(
        socket,
        PairResponse {
            error: Some(error),
            ..PairResponse::default()
        },
    )
    .await?;
    Ok(())
}

async fn write_response(socket: &mut TcpStream, response: PairResponse) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
    bytes.push(b'\n');
    socket.write_all(&bytes).await
}

fn is_lan_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local() || ip.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_private_link_local_or_loopback_addresses_can_pair() {
        assert!(is_lan_address("192.168.1.10".parse().unwrap()));
        assert!(is_lan_address("10.0.0.2".parse().unwrap()));
        assert!(is_lan_address("127.0.0.1".parse().unwrap()));
        assert!(!is_lan_address("8.8.8.8".parse().unwrap()));
    }

    async fn exchange(
        pairing: Arc<Mutex<PairingState>>,
        request: serde_json::Value,
    ) -> serde_json::Value {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, remote) = listener.accept().await.unwrap();
            handle_pair_request(socket, remote, pairing).await.unwrap();
        });
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        server.await.unwrap();
        serde_json::from_slice(&response).unwrap()
    }

    #[tokio::test]
    async fn tv_first_activation_persists_authorizes_and_polls_approved() {
        let dir = std::env::temp_dir().join(format!(
            "swarm-lan-activation-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(StateDb::open(&dir).await.unwrap());
        let allowed = AllowedPeers::new();
        let pairing = Arc::new(Mutex::new(PairingState::default()));
        let fingerprint = "ab".repeat(32);

        let started = exchange(
            Arc::clone(&pairing),
            serde_json::json!({
                "action": "begin",
                "name": "Living Room TV",
                "fingerprint": fingerprint,
            }),
        )
        .await;
        assert_eq!(started["ok"], true);
        assert_eq!(started["code"].as_str().unwrap().len(), 8);
        assert_eq!(started["status"], "pending");

        let approval =
            approve_pending_pairing(&pairing, &db, &allowed, started["code"].as_str().unwrap())
                .await
                .unwrap();
        assert_eq!(approval.name, "Living Room TV");
        assert!(allowed.contains(&fingerprint));
        assert_eq!(db.local_peers().await.unwrap()[0].fingerprint, fingerprint);

        let polled = exchange(
            Arc::clone(&pairing),
            serde_json::json!({
                "action": "poll",
                "activation_id": started["activation_id"],
                "poll_token": started["poll_token"],
            }),
        )
        .await;
        assert_eq!(polled["ok"], true);
        assert_eq!(polled["status"], "approved");

        drop(db);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repeated_begin_for_same_tv_reuses_one_pending_code() {
        let mut state = PairingState::default();
        let ip = "192.168.1.20".parse().unwrap();
        let first = state.begin("TV".into(), "ab".repeat(32), ip).unwrap();
        let second = state.begin("TV".into(), "ab".repeat(32), ip).unwrap();
        assert_eq!(first.code, second.code);
        assert_eq!(state.activations.len(), 1);
    }
}
