//! Proves the piece the whole hole-punch design depends on and that quinn's
//! docs only promise in the abstract: a socket used for raw `PUNCH_MAGIC`
//! traffic can be handed straight to quinn afterward, and a real QUIC
//! connection — full mTLS handshake, fingerprint pinning both directions,
//! an actual request/response — succeeds over that exact same 4-tuple.
//! `punch()` and `listen_on_socket`/`connect_on_socket` are each unit-tested
//! on their own; this is the thing that actually matters, which neither of
//! those tests can show by itself: that the handoff between them is real,
//! not just two pieces that each work in isolation.

use swarm_core::peer::{PeerRequest, PeerResponseHeader};
use swarm_p2p::endpoint::{connect_on_socket, listen_on_socket, read_body, read_request, send_request, write_response_header};
use swarm_p2p::identity::ensure_identity;
use swarm_p2p::pin::AllowedPeers;
use swarm_p2p::punch::punch;
use tokio::net::UdpSocket;

#[tokio::test]
async fn a_punched_socket_carries_a_real_pinned_quic_connection() {
    let base = std::env::temp_dir().join(format!("swarm-punch-to-quic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dialer_identity = ensure_identity(&base.join("dialer-id")).unwrap();
    let listener_identity = ensure_identity(&base.join("listener-id")).unwrap();

    let dialer_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listener_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dialer_addr = dialer_socket.local_addr().unwrap();
    let listener_addr = listener_socket.local_addr().unwrap();

    // The hole punch itself — on loopback there's no NAT to actually
    // traverse, but the mechanics (magic exchange, first-receipt-wins) are
    // the real thing, and critically this is the exact socket + port that
    // carries the QUIC traffic next.
    let (dialer_candidates, listener_candidates) = ([listener_addr], [dialer_addr]);
    let (dialer_punch, listener_punch) =
        tokio::join!(punch(&dialer_socket, &dialer_candidates), punch(&listener_socket, &listener_candidates));
    dialer_punch.unwrap();
    listener_punch.unwrap();

    let listener_allowed = AllowedPeers::new();
    listener_allowed.replace([dialer_identity.fingerprint.clone()]);

    let listener_endpoint =
        listen_on_socket(listener_socket.into_std().unwrap(), &listener_identity, listener_allowed).unwrap();
    let accept_task = tokio::spawn({
        let listener_endpoint = listener_endpoint.clone();
        async move {
            let incoming = listener_endpoint.accept().await.unwrap();
            let connection = incoming.await.unwrap();
            let (mut send, mut recv) = connection.accept_bi().await.unwrap();
            let request = read_request(&mut recv).await.unwrap();
            assert_eq!(request.path, "/ping");
            let header = PeerResponseHeader { status: 200, len: 4, content_type: None, content_range: None, etag: None };
            write_response_header(&mut send, &header).await.unwrap();
            send.write_all(b"pong").await.unwrap();
            send.finish().ok();
            // Don't drop `connection` (and tear the whole thing down,
            // possibly before the last bytes are acked) until the dialer
            // is done and closes its side — same lifecycle
            // `swarm_media::serve::accept_loop` relies on by looping
            // `accept_bi` until it errors, just simplified for one stream.
            let _ = connection.closed().await;
        }
    });

    let connection = connect_on_socket(
        dialer_socket.into_std().unwrap(),
        listener_addr,
        &dialer_identity,
        &listener_identity.fingerprint,
    )
    .await
    .unwrap();
    let request = PeerRequest { path: "/ping".into(), range: None, if_none_match: None, playback: None };
    let (header, mut recv) = send_request(&connection, &request).await.unwrap();
    assert_eq!(header.status, 200);
    let body = read_body(&header, &mut recv).await.unwrap();
    assert_eq!(&body, b"pong");

    // An explicit close, not just a drop: dropping a Connection handle
    // doesn't send a CONNECTION_CLOSE frame by itself, so the server's
    // `connection.closed().await` would otherwise sit there until quinn's
    // idle timeout (tens of seconds), not until this line runs.
    connection.close(0u32.into(), b"done");
    accept_task.await.unwrap();
}
