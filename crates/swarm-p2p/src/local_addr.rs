//! Local network address detection — "which of my interfaces would the OS
//! use to reach the outside world," via Batocera.Drone's zero-packet trick
//! (`network_identity.get_local_ip_addresses`): UDP `connect()` to a
//! well-known address and read back the socket's local address. UDP
//! `connect()` only consults the kernel's routing table to pick a source
//! address — it never actually sends a packet — so this works even fully
//! offline and carries no privacy/network cost.
//!
//! This is how a server self-reports the address peers should dial (via
//! `PATCH /api/v1/devices/{id}/metadata`, key `peer_addr`) — the STUN
//! roster otherwise has no way to tell a client *where* a server is, only
//! *that* it exists.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// A public DNS resolver, used only as a routing-table probe target — no
/// data is ever sent to it.
const PROBE_TARGET: &str = "8.8.8.8:80";

/// Best-guess LAN-facing IPv4 address. Falls back to loopback if no route
/// exists (offline, sandboxed, IPv6-only) — a serving address the local
/// device can always at least reach itself on, so callers never need to
/// handle "no address at all."
pub fn detect_local_ipv4() -> IpAddr {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect(PROBE_TARGET)?;
            socket.local_addr()
        })
        .map(|addr| addr.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

/// [`detect_local_ipv4`] combined with `port`, ready to publish as
/// self-reported connect metadata.
pub fn detect_local_addr(port: u16) -> SocketAddr {
    SocketAddr::new(detect_local_ipv4(), port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_non_unspecified_address() {
        let ip = detect_local_ipv4();
        assert_ne!(ip, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn combines_with_port() {
        let addr = detect_local_addr(8543);
        assert_eq!(addr.port(), 8543);
        assert_eq!(addr.ip(), detect_local_ipv4());
    }
}
