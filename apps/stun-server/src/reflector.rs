//! UDP reflexive-address reflector.
//!
//! Contract (byte-compatible with the retired Batocera.Drone Edge reflector):
//! a device sends the datagram `b"bind"`; the reflector replies with JSON
//! `{"ip": "<observed ip>", "port": <observed port>}` to the source address.
//! Anything else is ignored. No state, no auth — the reply only tells the
//! sender what the world already sees.

use swarm_core::signal::{ReflectorResponse, REFLECTOR_BIND_REQUEST};
use tokio::net::UdpSocket;

pub async fn run(port: u16) -> std::io::Result<()> {
    let socket = UdpSocket::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "reflector listening");
    let mut buf = [0u8; 64];
    loop {
        let Ok((len, from)) = socket.recv_from(&mut buf).await else {
            continue;
        };
        if &buf[..len] != REFLECTOR_BIND_REQUEST {
            continue;
        }
        let reply = ReflectorResponse { ip: from.ip().to_string(), port: from.port() };
        if let Ok(json) = serde_json::to_vec(&reply) {
            let _ = socket.send_to(&json, from).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reflects_source_address() {
        // Bind the reflector on an ephemeral port by racing run() against a probe.
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();
        drop(server); // free it for run(); tiny race is fine in a test
        let task = tokio::spawn(run(port));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(REFLECTOR_BIND_REQUEST, ("127.0.0.1", port)).await.unwrap();
        let mut buf = [0u8; 128];
        let (len, _) = tokio::time::timeout(std::time::Duration::from_secs(2), client.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let resp: ReflectorResponse = serde_json::from_slice(&buf[..len]).unwrap();
        assert_eq!(resp.ip, "127.0.0.1");
        assert_eq!(resp.port, client.local_addr().unwrap().port());
        task.abort();
    }
}
