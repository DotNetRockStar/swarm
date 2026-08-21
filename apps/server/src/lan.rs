//! STUN-free LAN discovery and pairing.
//!
//! The media server advertises its QUIC endpoint and certificate fingerprint
//! over mDNS. Discovery is intentionally not authorization: an Android TV
//! client must also submit its own certificate fingerprint with the six-digit
//! code shown by the server during a short pairing window. Successful peers
//! are persisted in `server-state.sqlite` and join the same `AllowedPeers`
//! set used by STUN roster members, so the existing mutual-TLS data path stays
//! unchanged.

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
const PAIRING_WINDOW: Duration = Duration::from_secs(5 * 60);
const MAX_PAIR_REQUEST: usize = 4096;

#[derive(Debug, Clone, Serialize)]
pub struct PairingStatus {
    pub code: String,
    pub expires_in_seconds: u64,
    pub pairing_port: u16,
}

#[derive(Default)]
struct PairingWindow {
    code: Option<String>,
    expires_at: Option<Instant>,
}

impl PairingWindow {
    fn matches(&self, code: &str) -> bool {
        self.code.as_deref() == Some(code)
            && self
                .expires_at
                .is_some_and(|deadline| deadline > Instant::now())
    }
}

struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

pub struct LanService {
    pairing: Arc<Mutex<PairingWindow>>,
    pairing_port: u16,
    advertisement: Option<Advertisement>,
}

impl LanService {
    pub async fn start(
        server_fingerprint: String,
        peer_addr: SocketAddr,
        allowed: AllowedPeers,
        state_db: Arc<StateDb>,
    ) -> std::io::Result<Self> {
        // TCP and QUIC/UDP can share the same numeric port. Keeping pairing
        // on the configured peer port avoids an unpredictable high port that
        // would be awkward to permit through a host firewall.
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, peer_addr.port())).await?;
        let pairing_port = listener.local_addr()?.port();
        let pairing = Arc::new(Mutex::new(PairingWindow::default()));
        let listener_pairing = Arc::clone(&pairing);
        tokio::spawn(async move {
            loop {
                let (socket, remote) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(err) => {
                        tracing::warn!(%err, "LAN pairing listener stopped");
                        return;
                    }
                };
                let window = Arc::clone(&listener_pairing);
                let db = Arc::clone(&state_db);
                let allowed = allowed.clone();
                tokio::spawn(async move {
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        handle_pair_request(socket, remote, window, db, allowed),
                    )
                    .await
                    {
                        Ok(Err(err)) => {
                            tracing::debug!(%remote, %err, "LAN pairing request failed")
                        }
                        Err(_) => tracing::debug!(%remote, "LAN pairing request timed out"),
                        Ok(Ok(())) => {}
                    }
                });
            }
        });

        let advertisement = advertise(&server_fingerprint, peer_addr, pairing_port);
        Ok(Self {
            pairing,
            pairing_port,
            advertisement,
        })
    }

    pub async fn open_pairing_window(&self) -> PairingStatus {
        let code = format!("{:06}", rand::random::<u32>() % 1_000_000);
        let mut window = self.pairing.lock().await;
        window.code = Some(code.clone());
        window.expires_at = Some(Instant::now() + PAIRING_WINDOW);
        PairingStatus {
            code,
            expires_in_seconds: PAIRING_WINDOW.as_secs(),
            pairing_port: self.pairing_port,
        }
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
) -> Option<Advertisement> {
    let daemon = ServiceDaemon::new()
        .map_err(|err| tracing::warn!(%err, "could not start mDNS advertiser"))
        .ok()?;
    let short = &server_fingerprint[..server_fingerprint.len().min(12)];
    let instance = format!("SWARM Media Server {short}");
    let hostname = format!("swarm-{short}.local.");
    let peer_port = peer_addr.port().to_string();
    let pair_port = pairing_port.to_string();
    let properties = [
        ("protocol", "1"),
        ("name", "SWARM Media Server"),
        ("fingerprint", server_fingerprint),
        ("peer_port", peer_port.as_str()),
        ("pair_port", pair_port.as_str()),
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
    tracing::info!(service = %fullname, pairing_port, "advertising media server on the LAN");
    Some(Advertisement { daemon, fullname })
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
    name: String,
    fingerprint: String,
}

#[derive(Serialize)]
struct PairResponse<'a> {
    ok: bool,
    error: Option<&'a str>,
}

async fn handle_pair_request(
    mut socket: TcpStream,
    remote: SocketAddr,
    pairing: Arc<Mutex<PairingWindow>>,
    state_db: Arc<StateDb>,
    allowed: AllowedPeers,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !is_lan_address(remote.ip()) {
        write_response(&mut socket, false, Some("not_lan")).await?;
        return Ok(());
    }
    let mut request_bytes = Vec::new();
    loop {
        if request_bytes.len() >= MAX_PAIR_REQUEST {
            write_response(&mut socket, false, Some("request_too_large")).await?;
            return Ok(());
        }
        let mut byte = [0u8; 1];
        if socket.read(&mut byte).await? == 0 || byte[0] == b'\n' {
            break;
        }
        request_bytes.push(byte[0]);
    }
    let request: PairRequest = match serde_json::from_slice(&request_bytes) {
        Ok(request) => request,
        Err(_) => {
            write_response(&mut socket, false, Some("invalid_request")).await?;
            return Ok(());
        }
    };
    let fingerprint = request.fingerprint.trim().to_lowercase();
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        write_response(&mut socket, false, Some("invalid_fingerprint")).await?;
        return Ok(());
    }
    let name = request.name.trim();
    if name.is_empty() || name.len() > 80 {
        write_response(&mut socket, false, Some("invalid_name")).await?;
        return Ok(());
    }
    {
        let mut window = pairing.lock().await;
        if !window.matches(request.code.trim()) {
            write_response(&mut socket, false, Some("pairing_closed_or_bad_code")).await?;
            return Ok(());
        }
        // One successful client per window prevents a code observed during
        // pairing from being reused by a second device.
        window.code = None;
        window.expires_at = None;
    }
    state_db.save_local_peer(&fingerprint, name).await?;
    allowed.insert(&fingerprint);
    tracing::info!(client = %name, fingerprint = %fingerprint, %remote, "paired local client");
    write_response(&mut socket, true, None).await?;
    Ok(())
}

async fn write_response(
    socket: &mut TcpStream,
    ok: bool,
    error: Option<&str>,
) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(&PairResponse { ok, error }).unwrap_or_default();
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

    #[tokio::test]
    async fn valid_pairing_persists_and_authorizes_the_client() {
        let dir = std::env::temp_dir().join(format!(
            "swarm-lan-pairing-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(StateDb::open(&dir).await.unwrap());
        let allowed = AllowedPeers::new();
        let pairing = Arc::new(Mutex::new(PairingWindow {
            code: Some("123456".into()),
            expires_at: Some(Instant::now() + Duration::from_secs(60)),
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = {
            let db = Arc::clone(&db);
            let allowed = allowed.clone();
            tokio::spawn(async move {
                let (socket, remote) = listener.accept().await.unwrap();
                handle_pair_request(socket, remote, pairing, db, allowed)
                    .await
                    .unwrap();
            })
        };

        let fingerprint = "ab".repeat(32);
        let request = serde_json::json!({
            "code": "123456",
            "name": "Living Room TV",
            "fingerprint": fingerprint,
        });
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        server.await.unwrap();

        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&response).unwrap()["ok"],
            true
        );
        assert!(allowed.contains(&fingerprint));
        assert_eq!(db.local_peers().await.unwrap()[0].fingerprint, fingerprint);
        drop(db);
        std::fs::remove_dir_all(&dir).ok();
    }
}
