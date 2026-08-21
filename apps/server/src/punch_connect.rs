//! Ties `swarm_p2p`'s hole-punch primitives (`reflector`, `punch`,
//! `endpoint::{listen_on_socket, connect_on_socket}`) to
//! `swarm_stun_client::SignalingClient` into one "establish a connection to
//! this peer" flow — `docs/PROTOCOL.md`'s connection-establishment section,
//! made concrete. Lives here rather than in either of those crates because
//! it needs both in scope; keeping the primitives crate-independent means
//! this is the only place that pays the coupling cost, and only for the one
//! binary that actually needs it.
//!
//! Two entry points, matching the protocol doc's "initiator I / responder
//! R": [`initiate_punch_connection`] sends the `Offer` and ends up dialing
//! QUIC; [`respond_to_punch_offer`] reacts to an already-received `Offer`,
//! sends the `Answer`, and ends up accepting. Both gather the same
//! candidates, punch, and require mutual `Punched` confirmation before
//! touching QUIC at all — mirrors the protocol doc's "neither side switches
//! to the punched path while the other is still waiting."
//!
//! **Known simplification.** Both functions borrow the caller's signaling
//! inbound receiver exclusively for the duration of one attempt — there's
//! no demuxing layer for running a punch concurrently with, say, watching
//! background presence updates on the same signaling session. A punch
//! attempt is short (bounded by `punch`'s own ~4s ceiling plus a couple of
//! signaling round trips), so a caller doing one thing at a time with its
//! signaling connection is a reasonable cost for a first version — revisit
//! if a real use case needs to interleave.
//!
//! No UPnP/NAT-PMP yet, so candidate gathering never produces a `forwarded`
//! candidate and the protocol doc's "responder has a forwarded candidate,
//! skip punching" shortcut never fires — every connection here punches.

use rand::RngCore;
use std::net::SocketAddr;
use std::time::Duration;
use swarm_core::signal::{Candidate, CandidateKind, SignalMessage, SignalPayload};
use swarm_p2p::identity::DeviceIdentity;
use swarm_p2p::pin::AllowedPeers;
use swarm_stun_client::{SignalingClient, SignalingError};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
const CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum PunchConnectError {
    #[error("socket error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not learn our reflexive address: {0}")]
    Reflector(#[from] swarm_p2p::reflector::ReflectorError),
    #[error("could not send a signal message: {0}")]
    Signaling(#[from] SignalingError),
    #[error("timed out waiting for the peer's answer")]
    AnswerTimeout,
    #[error("timed out waiting for mutual punch confirmation")]
    ConfirmationTimeout,
    #[error("timed out waiting for the peer's QUIC connection")]
    AcceptTimeout,
    #[error("signaling connection closed mid-attempt")]
    SignalingClosed,
    #[error("peer reported a signaling error ({code}): {message}")]
    PeerError { code: String, message: String },
    #[error("peer's answer certificate fingerprint didn't match the pinned roster entry (expected {expected}, got {actual})")]
    FingerprintMismatch { expected: String, actual: String },
    #[error("no usable candidates to punch toward")]
    NoCandidates,
    #[error("hole punch failed: {0}")]
    Punch(#[from] swarm_p2p::punch::PunchError),
    #[error("quic connection failed: {0}")]
    Quic(#[from] swarm_p2p::endpoint::P2pError),
}

/// An `Offer` already pulled off the caller's own signal receiver —
/// extracting it from the general inbound stream (alongside presence
/// updates etc.) is the caller's job, this module only takes over once
/// something has decided an incoming message is an offer worth acting on.
pub struct ReceivedOffer {
    pub from: String,
    pub punch_id: String,
    pub candidates: Vec<Candidate>,
    pub cert_fingerprint: String,
}

fn random_punch_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

async fn gather_candidates(
    socket: &UdpSocket,
    reflector_addr: SocketAddr,
) -> Result<Vec<Candidate>, PunchConnectError> {
    let mut candidates = Vec::new();
    // `socket.local_addr()` would report the wildcard bind address
    // (0.0.0.0) we bound to, not a real routable interface — useless, in
    // fact actively wrong, as something to hand to a peer. Only the port
    // comes from the socket; the address comes from the same route-table
    // probe `local_addr::detect_local_addr` already uses for `peer_addr`.
    let port = socket.local_addr()?.port();
    let local = swarm_p2p::local_addr::detect_local_addr(port);
    candidates.push(Candidate {
        kind: CandidateKind::Lan,
        ip: local.ip().to_string(),
        port: local.port(),
    });
    if let Ok(reflexive) = swarm_p2p::reflector::reflexive_addr(socket, reflector_addr).await {
        candidates.push(Candidate {
            kind: CandidateKind::Reflexive,
            ip: reflexive.ip().to_string(),
            port: reflexive.port(),
        });
    }
    Ok(candidates)
}

/// LAN candidates first, matching the protocol doc's punch ordering.
fn candidate_addrs(candidates: &[Candidate]) -> Vec<SocketAddr> {
    let mut sorted: Vec<&Candidate> = candidates.iter().collect();
    sorted.sort_by_key(|c| match c.kind {
        CandidateKind::Lan => 0,
        CandidateKind::Forwarded => 1,
        CandidateKind::Reflexive => 2,
    });
    sorted
        .iter()
        .filter_map(|c| format!("{}:{}", c.ip, c.port).parse().ok())
        .collect()
}

/// Waits for a `Signal` from `from_device` whose payload `matches` accepts,
/// ignoring anything else (presence, signals from other peers, non-matching
/// payloads) that arrives in the meantime — this attempt only cares about
/// its own negotiation. An `Error` frame from the server is treated as
/// fatal immediately rather than ignored, since it means the peer (or the
/// server on the peer's behalf) rejected something about this attempt.
async fn await_signal_payload<T>(
    rx: &mut mpsc::UnboundedReceiver<SignalMessage>,
    from_device: &str,
    timeout: Duration,
    on_timeout: PunchConnectError,
    mut matches: impl FnMut(&SignalPayload) -> Option<T>,
) -> Result<T, PunchConnectError> {
    let wait = async {
        loop {
            match rx.recv().await.ok_or(PunchConnectError::SignalingClosed)? {
                SignalMessage::Signal { from, payload, .. }
                    if from.as_deref() == Some(from_device) =>
                {
                    if let Some(value) = matches(&payload) {
                        return Ok(value);
                    }
                }
                SignalMessage::Error { code, message } => {
                    return Err(PunchConnectError::PeerError { code, message })
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(timeout, wait)
        .await
        .unwrap_or(Err(on_timeout))
}

/// The "I" (initiator) role: offers, waits for an answer pinned to
/// `expected_fingerprint`, punches to the answered candidates, confirms
/// mutually, and dials QUIC over the punched socket.
pub async fn initiate_punch_connection(
    signaling: &SignalingClient,
    signal_rx: &mut mpsc::UnboundedReceiver<SignalMessage>,
    reflector_addr: SocketAddr,
    peer_device_id: &str,
    identity: &DeviceIdentity,
    expected_fingerprint: &str,
) -> Result<quinn::Connection, PunchConnectError> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let candidates = gather_candidates(&socket, reflector_addr).await?;
    let punch_id = random_punch_id();

    signaling.send_signal(
        peer_device_id,
        SignalPayload::Offer {
            punch_id: punch_id.clone(),
            candidates,
            cert_fingerprint: identity.fingerprint.clone(),
        },
    )?;

    let expected_punch_id = punch_id.clone();
    let (answer_candidates, answer_fingerprint) = await_signal_payload(
        signal_rx,
        peer_device_id,
        ANSWER_TIMEOUT,
        PunchConnectError::AnswerTimeout,
        |payload| match payload {
            SignalPayload::Answer {
                punch_id,
                candidates,
                cert_fingerprint,
            } if *punch_id == expected_punch_id => {
                Some((candidates.clone(), cert_fingerprint.clone()))
            }
            _ => None,
        },
    )
    .await?;
    if answer_fingerprint != expected_fingerprint {
        return Err(PunchConnectError::FingerprintMismatch {
            expected: expected_fingerprint.to_string(),
            actual: answer_fingerprint,
        });
    }

    let targets = candidate_addrs(&answer_candidates);
    if targets.is_empty() {
        return Err(PunchConnectError::NoCandidates);
    }
    let confirmed_addr = swarm_p2p::punch::punch(&socket, &targets).await?;
    signaling.send_signal(
        peer_device_id,
        SignalPayload::Punched {
            punch_id: punch_id.clone(),
            ok: true,
        },
    )?;

    let expected_punch_id = punch_id.clone();
    await_signal_payload(
        signal_rx,
        peer_device_id,
        CONFIRMATION_TIMEOUT,
        PunchConnectError::ConfirmationTimeout,
        |payload| match payload {
            SignalPayload::Punched { punch_id, ok: true } if *punch_id == expected_punch_id => {
                Some(())
            }
            _ => None,
        },
    )
    .await?;

    let connection = swarm_p2p::endpoint::connect_on_socket(
        socket.into_std()?,
        confirmed_addr,
        identity,
        expected_fingerprint,
    )
    .await?;
    Ok(connection)
}

/// The "R" (responder) role: answers an already-received `offer`, punches
/// to its candidates, confirms mutually, and accepts the initiator's QUIC
/// dial over the punched socket. `allowed` gates who the resulting QUIC
/// accept trusts — same roster-membership check `listen`/`accept_loop` use
/// for every other inbound peer connection, not something this function
/// weakens or bypasses.
pub async fn respond_to_punch_offer(
    signaling: &SignalingClient,
    signal_rx: &mut mpsc::UnboundedReceiver<SignalMessage>,
    reflector_addr: SocketAddr,
    offer: ReceivedOffer,
    identity: &DeviceIdentity,
    allowed: AllowedPeers,
) -> Result<quinn::Connection, PunchConnectError> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let candidates = gather_candidates(&socket, reflector_addr).await?;

    signaling.send_signal(
        &offer.from,
        SignalPayload::Answer {
            punch_id: offer.punch_id.clone(),
            candidates,
            cert_fingerprint: identity.fingerprint.clone(),
        },
    )?;

    let targets = candidate_addrs(&offer.candidates);
    if targets.is_empty() {
        return Err(PunchConnectError::NoCandidates);
    }
    swarm_p2p::punch::punch(&socket, &targets).await?;
    signaling.send_signal(
        &offer.from,
        SignalPayload::Punched {
            punch_id: offer.punch_id.clone(),
            ok: true,
        },
    )?;

    let expected_punch_id = offer.punch_id.clone();
    await_signal_payload(
        signal_rx,
        &offer.from,
        CONFIRMATION_TIMEOUT,
        PunchConnectError::ConfirmationTimeout,
        |payload| match payload {
            SignalPayload::Punched { punch_id, ok: true } if *punch_id == expected_punch_id => {
                Some(())
            }
            _ => None,
        },
    )
    .await?;

    let endpoint = swarm_p2p::endpoint::listen_on_socket(socket.into_std()?, identity, allowed)?;
    let incoming = tokio::time::timeout(ACCEPT_TIMEOUT, endpoint.accept())
        .await
        .map_err(|_| PunchConnectError::AcceptTimeout)?
        .ok_or(PunchConnectError::AcceptTimeout)?;
    let connection = incoming
        .await
        .map_err(swarm_p2p::endpoint::P2pError::from)?;
    Ok(connection)
}
