//! quinn QUIC endpoints wired to the fingerprint-pinning verifiers, plus the
//! peer request/response framing from `swarm-core::peer`.
//!
//! Wire shape (one request per bidirectional stream, HTTP-shaped on purpose):
//! initiator writes one JSON line (`PeerRequest`) and finishes its send side;
//! responder writes one JSON line (`PeerResponseHeader`) followed by exactly
//! `len` body bytes.

use crate::identity::DeviceIdentity;
use crate::pin::{AllowedPeers, PinnedServerVerifier, RosterClientVerifier};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::net::SocketAddr;
use std::sync::Arc;
use swarm_core::peer::{PeerRequest, PeerResponseHeader};

/// ALPN for the SWARM peer protocol; version-bumped with the wire format.
pub const ALPN: &[u8] = b"swarm-peer/1";

const MAX_HEADER_LINE: usize = 64 * 1024;

/// Fingerprint the certificate authenticated for an established peer.
/// Returns `None` only for a transport without rustls certificate identity.
pub fn peer_fingerprint(connection: &quinn::Connection) -> Option<String> {
    let identity = connection.peer_identity()?;
    let certificates = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    certificates
        .first()
        .map(|certificate| crate::identity::fingerprint_der(certificate.as_ref()))
}

#[derive(Debug, thiserror::Error)]
pub enum P2pError {
    #[error("tls setup failed: {0}")]
    Tls(#[from] rustls::Error),
    #[error("quic setup failed: {0}")]
    Quic(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("connection failed: {0}")]
    Connect(#[from] quinn::ConnectionError),
    #[error("stream write failed: {0}")]
    Write(#[from] quinn::WriteError),
    #[error("stream read failed: {0}")]
    Read(#[from] quinn::ReadError),
    #[error("malformed peer message: {0}")]
    Protocol(String),
}

fn identity_material(
    identity: &DeviceIdentity,
) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let chain = vec![CertificateDer::from(identity.cert_der.clone())];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key_der.clone()));
    (chain, key)
}

/// Shared QUIC transport tuning. quinn's stock 30s idle timeout with no
/// keep-alive drops a connection that goes quiet — which a peer request
/// legitimately does while the far side negotiates playback (a large transcode
/// can take a minute to flush its first HLS playlist off a slow share) or
/// while a viewer sits on the pause overlay between segment fetches. Server-
/// sent keep-alive PINGs hold the path open through those gaps for both ends.
fn peer_transport_config() -> Arc<quinn::TransportConfig> {
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
    transport.max_idle_timeout(Some(
        std::time::Duration::from_secs(90)
            .try_into()
            .expect("90s is a valid QUIC idle timeout"),
    ));
    Arc::new(transport)
}

fn server_config(
    identity: &DeviceIdentity,
    allowed: AllowedPeers,
) -> Result<quinn::ServerConfig, P2pError> {
    let (chain, key) = identity_material(identity);
    let mut tls = rustls::ServerConfig::builder()
        .with_client_cert_verifier(RosterClientVerifier::new(allowed))
        .with_single_cert(chain, key)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let quic = QuicServerConfig::try_from(tls).map_err(|e| P2pError::Quic(e.to_string()))?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(quic));
    config.transport_config(peer_transport_config());
    Ok(config)
}

fn client_config(
    identity: &DeviceIdentity,
    expected_fingerprint: &str,
) -> Result<quinn::ClientConfig, P2pError> {
    let (chain, key) = identity_material(identity);
    let mut tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(PinnedServerVerifier::new(expected_fingerprint))
        .with_client_auth_cert(chain, key)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let quic = QuicClientConfig::try_from(tls).map_err(|e| P2pError::Quic(e.to_string()))?;
    let mut config = quinn::ClientConfig::new(Arc::new(quic));
    config.transport_config(peer_transport_config());
    Ok(config)
}

/// Accepting endpoint for the server role: requires a client cert whose
/// fingerprint is in `allowed` (the live swarm roster).
pub fn listen(
    bind: SocketAddr,
    identity: &DeviceIdentity,
    allowed: AllowedPeers,
) -> Result<quinn::Endpoint, P2pError> {
    Ok(quinn::Endpoint::server(
        server_config(identity, allowed)?,
        bind,
    )?)
}

/// Dial a peer, verifying its certificate against `expected_fingerprint` and
/// presenting our own identity for the roster check on the far side.
pub async fn connect(
    remote: SocketAddr,
    identity: &DeviceIdentity,
    expected_fingerprint: &str,
) -> Result<quinn::Connection, P2pError> {
    let bind: SocketAddr = if remote.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(client_config(identity, expected_fingerprint)?);
    // SNI is irrelevant under pinning but rustls requires a name; use a fixed one.
    let connection = endpoint
        .connect(remote, "swarm-peer")
        .map_err(|e| P2pError::Quic(e.to_string()))?
        .await?;
    Ok(connection)
}

/// Like [`listen`], but takes ownership of a socket the caller already used
/// — a hole punch via [`crate::punch::punch`], most likely — instead of
/// binding a fresh one. QUIC traffic then continues on the exact 4-tuple
/// whatever NAT mapping the punch opened is valid for; a new socket would
/// get a new local port and throw that mapping away.
pub fn listen_on_socket(
    socket: std::net::UdpSocket,
    identity: &DeviceIdentity,
    allowed: AllowedPeers,
) -> Result<quinn::Endpoint, P2pError> {
    Ok(quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config(identity, allowed)?),
        socket,
        Arc::new(quinn::TokioRuntime),
    )?)
}

/// Like [`connect`], but dials from a socket the caller already used —
/// see [`listen_on_socket`] for why this matters for a punched connection.
pub async fn connect_on_socket(
    socket: std::net::UdpSocket,
    remote: SocketAddr,
    identity: &DeviceIdentity,
    expected_fingerprint: &str,
) -> Result<quinn::Connection, P2pError> {
    let mut endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        None,
        socket,
        Arc::new(quinn::TokioRuntime),
    )?;
    endpoint.set_default_client_config(client_config(identity, expected_fingerprint)?);
    let connection = endpoint
        .connect(remote, "swarm-peer")
        .map_err(|e| P2pError::Quic(e.to_string()))?
        .await?;
    Ok(connection)
}

/// Read one `\n`-terminated JSON line from a stream, bounded by
/// [`MAX_HEADER_LINE`].
async fn read_json_line<T: serde::de::DeserializeOwned>(
    recv: &mut quinn::RecvStream,
) -> Result<T, P2pError> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match recv.read(&mut byte).await? {
            Some(1) => {
                if byte[0] == b'\n' {
                    break;
                }
                line.push(byte[0]);
                if line.len() > MAX_HEADER_LINE {
                    return Err(P2pError::Protocol("header line too long".into()));
                }
            }
            _ => return Err(P2pError::Protocol("stream ended before header line".into())),
        }
    }
    serde_json::from_slice(&line).map_err(|e| P2pError::Protocol(e.to_string()))
}

/// Client half: send a request, get the response header and the stream
/// positioned at the body (exactly `header.len` bytes follow).
pub async fn send_request(
    connection: &quinn::Connection,
    request: &PeerRequest,
) -> Result<(PeerResponseHeader, quinn::RecvStream), P2pError> {
    let (mut send, mut recv) = connection.open_bi().await.map_err(P2pError::Connect)?;
    let mut line = serde_json::to_vec(request).map_err(|e| P2pError::Protocol(e.to_string()))?;
    line.push(b'\n');
    send.write_all(&line).await?;
    send.finish().ok();
    let header: PeerResponseHeader = read_json_line(&mut recv).await?;
    Ok((header, recv))
}

/// Read a full response body of `header.len` bytes.
pub async fn read_body(
    header: &PeerResponseHeader,
    recv: &mut quinn::RecvStream,
) -> Result<Vec<u8>, P2pError> {
    let mut body = vec![0u8; header.len as usize];
    let mut filled = 0;
    while filled < body.len() {
        match recv.read(&mut body[filled..]).await? {
            Some(n) => filled += n,
            None => {
                return Err(P2pError::Protocol(format!(
                    "body truncated at {filled}/{} bytes",
                    body.len()
                )))
            }
        }
    }
    Ok(body)
}

/// Server half: read the request line from an accepted stream.
pub async fn read_request(recv: &mut quinn::RecvStream) -> Result<PeerRequest, P2pError> {
    read_json_line(recv).await
}

/// Server half: write the response header line; body bytes follow on `send`.
pub async fn write_response_header(
    send: &mut quinn::SendStream,
    header: &PeerResponseHeader,
) -> Result<(), P2pError> {
    let mut line = serde_json::to_vec(header).map_err(|e| P2pError::Protocol(e.to_string()))?;
    line.push(b'\n');
    send.write_all(&line).await?;
    Ok(())
}
