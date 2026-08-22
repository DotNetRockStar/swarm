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
//!
//! **Known, confirmed-on-real-hardware blind spot**: a full-tunnel VPN
//! captures the default route, so the probe above reports the VPN's
//! internal address (e.g. `10.2.0.2` on a macOS `utun` interface) instead
//! of the real LAN address a same-network peer needs to dial. Detected and
//! routed around below rather than left as a silent wrong answer — this
//! isn't hypothetical, it was caught live via a real server's self-reported
//! `peer_addr` while running behind an active corporate VPN.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// A public DNS resolver, used only as a routing-table probe target — no
/// data is ever sent to it.
const PROBE_TARGET: &str = "8.8.8.8:80";

/// Interface name prefixes used by VPN/tunnel adapters — `utun`/`ppp`
/// (macOS; virtually every VPN client, including corporate ones and
/// Tailscale/WireGuard, tunnels through `utun` there), `tun`/`tap`/`wg`
/// (Linux OpenVPN/WireGuard). Never a real physical LAN adapter, so an
/// address on one of these is never what a LAN peer should dial.
const TUNNEL_PREFIXES: &[&str] = &["utun", "tun", "tap", "ppp", "wg"];

fn is_tunnel_interface(name: &str) -> bool {
    TUNNEL_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

fn probe_via_routing_table() -> Option<IpAddr> {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect(PROBE_TARGET)?;
            socket.local_addr()
        })
        .map(|addr| addr.ip())
        .ok()
}

/// Best-guess LAN-facing IPv4 address. Falls back to loopback if no route
/// exists (offline, sandboxed, IPv6-only) — a serving address the local
/// device can always at least reach itself on, so callers never need to
/// handle "no address at all."
pub fn detect_local_ipv4() -> IpAddr {
    let probed = probe_via_routing_table();

    // The routing-table probe is the primary signal (cheap, portable, no
    // interface enumeration needed) — but only trust it once confirmed
    // it's not sitting on a tunnel interface.
    let probe_is_on_tunnel = probed.is_some_and(|ip| {
        if_addrs::get_if_addrs()
            .into_iter()
            .flatten()
            .any(|iface| iface.ip() == ip && is_tunnel_interface(&iface.name))
    });
    if !probe_is_on_tunnel {
        if let Some(ip) = probed {
            return ip;
        }
    }

    // Probe was on a tunnel (or failed outright) — fall back to the first
    // real, non-loopback, non-tunnel IPv4 interface.
    if_addrs::get_if_addrs()
        .into_iter()
        .flatten()
        .filter(|iface| !iface.is_loopback() && !is_tunnel_interface(&iface.name))
        .find_map(|iface| match iface.ip() {
            IpAddr::V4(v4) => Some(IpAddr::V4(v4)),
            IpAddr::V6(_) => None,
        })
        .or(probed)
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

    /// Real regression coverage, not hypothetical: run on a machine with an
    /// active full-tunnel VPN (a `utun` interface owning the default
    /// route), this test fails against the pre-fix implementation — it
    /// reports the VPN's internal address instead of the real LAN one.
    /// Passes regardless of whether a VPN happens to be active right now
    /// (nothing to assert if `detect_local_ipv4` isn't on any enumerated
    /// interface at all, e.g. the loopback fallback), but it's the one
    /// case this whole module exists to get right.
    #[test]
    fn never_reports_an_address_that_belongs_to_a_tunnel_interface() {
        let reported = detect_local_ipv4();
        let on_a_tunnel = if_addrs::get_if_addrs()
            .into_iter()
            .flatten()
            .any(|iface| iface.ip() == reported && is_tunnel_interface(&iface.name));
        assert!(
            !on_a_tunnel,
            "detect_local_ipv4() reported {reported}, which belongs to a tunnel interface"
        );
    }

    #[test]
    fn combines_with_port() {
        let addr = detect_local_addr(8543);
        assert_eq!(addr.port(), 8543);
        assert_eq!(addr.ip(), detect_local_ipv4());
    }
}
