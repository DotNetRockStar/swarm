//! Simultaneous UDP hole punching (`docs/PROTOCOL.md`'s connection-
//! establishment step 4): both sides blast `PUNCH_MAGIC` at every candidate
//! address, LAN first, while listening on the same socket for the same
//! magic coming back the other way.
//!
//! Deliberately signaling-agnostic: [`punch`] only proves *this device* can
//! receive from one of `candidates` — it doesn't know about
//! `swarm_stun_client::SignalingClient` or send `Punched` itself. Mutual
//! confirmation (so neither side switches to the punched path while the
//! other is still waiting, per the protocol doc) is the caller's job,
//! sitting one layer up where both this crate and the signaling client are
//! already in scope — matches how `apps/server`'s `ServerCore` is the thing
//! that wires `swarm-stun-client` and `swarm-p2p` together, not either
//! crate depending on the other.

use std::net::SocketAddr;
use std::time::Duration;
use swarm_core::signal::PUNCH_MAGIC;
use tokio::net::UdpSocket;
use tokio::time::Instant;

pub const MAX_ATTEMPTS: u32 = 20;
pub const ATTEMPT_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
pub enum PunchError {
    #[error("no response from any of {0} candidate(s) after {1} attempts")]
    NoResponse(usize, u32),
    #[error("socket error while punching: {0}")]
    Network(#[from] std::io::Error),
}

/// Tries every address in `candidates` (in the given order, each round —
/// ordering LAN-first is the caller's job) every [`ATTEMPT_INTERVAL`], up to
/// [`MAX_ATTEMPTS`] times, while listening on `socket` for the same magic
/// coming back. Returns the source address of the first valid magic packet
/// received from a listed candidate, having already sent one more magic
/// packet back to that exact address on the way out — the scheduled blast
/// for this round may have gone out to a different candidate that didn't
/// pan out, so this makes sure a packet flows back down the *confirmed*
/// path immediately rather than waiting for the next round.
pub async fn punch(socket: &UdpSocket, candidates: &[SocketAddr]) -> Result<SocketAddr, PunchError> {
    if candidates.is_empty() {
        return Err(PunchError::NoResponse(0, 0));
    }
    let mut buf = [0u8; PUNCH_MAGIC.len()];
    for _attempt in 0..MAX_ATTEMPTS {
        for &candidate in candidates {
            socket.send_to(PUNCH_MAGIC, candidate).await?;
        }
        let deadline = Instant::now() + ATTEMPT_INTERVAL;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
                Ok(Ok((len, from))) if &buf[..len] == PUNCH_MAGIC && candidates.contains(&from) => {
                    let _ = socket.send_to(PUNCH_MAGIC, from).await; // best-effort immediate reply
                    return Ok(from);
                }
                Ok(Ok(_)) => continue,       // stray/malformed datagram — keep waiting out this round
                Ok(Err(e)) => return Err(PunchError::Network(e)),
                Err(_) => break,             // this round's window elapsed — try the next round
            }
        }
    }
    Err(PunchError::NoResponse(candidates.len(), MAX_ATTEMPTS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn simultaneous_punch_succeeds_both_directions() {
        let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let a_addr = a.local_addr().unwrap();
        let b_addr = b.local_addr().unwrap();

        let (a_candidates, b_candidates) = ([b_addr], [a_addr]);
        let (a_result, b_result) = tokio::join!(punch(&a, &a_candidates), punch(&b, &b_candidates));
        assert_eq!(a_result.unwrap(), b_addr);
        assert_eq!(b_result.unwrap(), a_addr);
    }

    #[tokio::test]
    async fn tries_every_candidate_each_round_not_just_the_first() {
        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let listener_addr = listener.local_addr().unwrap();
        let dead_candidate: SocketAddr = "127.0.0.1:1".parse().unwrap(); // nothing responds here

        let puncher = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        let listener_task = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let (len, from) = listener.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..len], PUNCH_MAGIC);
            listener.send_to(PUNCH_MAGIC, from).await.unwrap();
        });

        let result = punch(&puncher, &[dead_candidate, listener_addr]).await.unwrap();
        assert_eq!(result, listener_addr);
        listener_task.await.unwrap();
    }

    #[tokio::test]
    async fn no_response_is_a_no_response_error() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let nobody: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let err = punch(&socket, &[nobody]).await.unwrap_err();
        assert!(matches!(err, PunchError::NoResponse(1, MAX_ATTEMPTS)));
    }

    #[tokio::test]
    async fn magic_from_an_unlisted_address_is_ignored() {
        let puncher = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let intended_candidate: SocketAddr = "127.0.0.1:1".parse().unwrap(); // never actually responds

        // A third party sends valid magic, but wasn't in the candidate list.
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let puncher_addr = puncher.local_addr().unwrap();
        tokio::spawn(async move {
            stranger.send_to(PUNCH_MAGIC, puncher_addr).await.unwrap();
        });

        let err = punch(&puncher, &[intended_candidate]).await.unwrap_err();
        assert!(matches!(err, PunchError::NoResponse(..)));
    }
}
