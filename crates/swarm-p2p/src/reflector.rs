//! UDP reflexive-address discovery client — the device side of
//! `apps/stun-server`'s reflector (`docs/PROTOCOL.md`'s "Reflexive address
//! discovery"): send `b"bind"`, the reflector replies with the sender's
//! observed `{ip, port}`. This is how a device learns its own public
//! (NATed) address to offer as a `reflexive` candidate during hole punching
//! — [`crate::local_addr`] only knows the LAN-local address, which is
//! useless once a peer is behind a different NAT.

use std::net::SocketAddr;
use std::time::Duration;
use swarm_core::signal::{ReflectorResponse, REFLECTOR_BIND_REQUEST};
use tokio::net::UdpSocket;

/// A single request/response is expected to be near-instant on a healthy
/// path; retry policy (try the fallback reflector port, try again) is a
/// concern for the candidate-gathering code that calls this, not this
/// primitive.
const REFLECTOR_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub enum ReflectorError {
    #[error("could not reach the reflector: {0}")]
    Network(#[from] std::io::Error),
    #[error("reflector did not respond within the timeout")]
    Timeout,
    #[error("could not parse the reflector's response: {0}")]
    Decode(String),
}

/// Sends `bind` to `reflector_addr` on `socket` and returns the address the
/// reflector observed us at. Takes a socket the caller already owns, rather
/// than binding a fresh one internally, so the same local port can be reused
/// for the actual hole punch afterward — the reflexive mapping a NAT hands
/// out is only valid for the 4-tuple it was observed on; a new socket would
/// get a new external port and invalidate the whole point of asking.
pub async fn reflexive_addr(socket: &UdpSocket, reflector_addr: SocketAddr) -> Result<SocketAddr, ReflectorError> {
    socket.send_to(REFLECTOR_BIND_REQUEST, reflector_addr).await?;
    let mut buf = [0u8; 512];
    let (len, from) = tokio::time::timeout(REFLECTOR_TIMEOUT, socket.recv_from(&mut buf))
        .await
        .map_err(|_| ReflectorError::Timeout)??;
    if from != reflector_addr {
        // Cheap sanity check, not real authentication: UDP has no session,
        // so don't trust the first datagram to land on this socket unless
        // it at least claims to be from the address we queried.
        return Err(ReflectorError::Decode(format!("reply from unexpected address {from}")));
    }
    let response: ReflectorResponse =
        serde_json::from_slice(&buf[..len]).map_err(|e| ReflectorError::Decode(e.to_string()))?;
    let ip: std::net::IpAddr =
        response.ip.parse().map_err(|_| ReflectorError::Decode(format!("invalid ip in reply: {}", response.ip)))?;
    Ok(SocketAddr::new(ip, response.port))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn spawn_reflector() -> SocketAddr {
        let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe); // free it for stun_server::reflector::run; tiny race is fine in a test
        tokio::spawn(stun_server::reflector::run(port));
        tokio::time::sleep(Duration::from_millis(50)).await;
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    #[tokio::test]
    async fn learns_its_own_observed_address_from_the_real_reflector() {
        let reflector = spawn_reflector().await;
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let local_port = socket.local_addr().unwrap().port();

        let observed = reflexive_addr(&socket, reflector).await.unwrap();
        assert_eq!(observed.ip().to_string(), "127.0.0.1");
        assert_eq!(observed.port(), local_port);
    }

    #[tokio::test]
    async fn nothing_listening_is_a_timeout_or_network_error() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let nobody: SocketAddr = "127.0.0.1:1".parse().unwrap(); // privileged, unbound in any test env
        let result = reflexive_addr(&socket, nobody).await;
        assert!(matches!(result, Err(ReflectorError::Timeout) | Err(ReflectorError::Network(_))));
    }
}
