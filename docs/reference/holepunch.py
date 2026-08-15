"""UDP NAT hole-punch primitives.

Two drones in different homes (different public IPs, both behind NAT) can often
open a direct UDP path without any port-forward:

1. Each creates a UDP socket and learns that socket's public ``ip:port`` from the
   Edge STUN reflector (:func:`gather_udp_candidate`).
2. They swap those candidates over the mux (MSG_SIGNAL).
3. Both send packets to each other's candidate at the same time; the first NAT to
   send creates an outbound mapping that lets the other's packets in
   (:func:`hole_punch`).

This module is just the path-establishment layer (no reliability). Carrying an
actual asset over the punched socket needs a reliable, ordered channel; see the
note in :class:`HolePunchUnavailable`. Symmetric NATs defeat hole punching
entirely -- callers fall back to the relay.
"""

from __future__ import annotations

import json
import socket
from typing import Any, Callable, Optional, Tuple

from .reliable_udp import ReliableUDPChannel

PUNCH_MAGIC = b"BFF-PUNCH"


class HolePunchUnavailable(RuntimeError):
    """Raised when a direct UDP path could not be established (e.g. symmetric
    NAT). The caller should fall back to the relay transport."""


def parse_addr(value: str) -> Tuple[str, int]:
    """Parse ``"ip:port"`` (IPv4 or ``[ipv6]:port``) into ``(host, port)``."""
    text = str(value or "").strip()
    if text.startswith("["):  # [ipv6]:port
        host, _, port = text[1:].partition("]:")
        return host, int(port)
    host, _, port = text.rpartition(":")
    if not host:
        raise ValueError(f"invalid address: {value!r}")
    return host, int(port)


def gather_udp_candidate(
    stun_addr: Tuple[str, int],
    *,
    timeout: float = 3.0,
    bind_addr: Tuple[str, int] = ("0.0.0.0", 0),
) -> Tuple[socket.socket, str]:
    """Create a UDP socket and learn its reflexive ``ip:port`` from the Edge STUN
    reflector. Returns ``(socket, reflexive_addr)``; the socket stays open for the
    subsequent punch. Raises on timeout / bad reply (caller falls back to relay)."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        sock.bind(bind_addr)
        sock.settimeout(timeout)
        sock.sendto(b"bind", stun_addr)
        data, _ = sock.recvfrom(512)
        info = json.loads(data.decode("utf-8"))
        reflexive = f"{info['ip']}:{int(info['port'])}"
    except (OSError, ValueError, KeyError) as error:
        sock.close()
        raise HolePunchUnavailable(f"STUN candidate gathering failed: {error}") from error
    return sock, reflexive


def hole_punch(
    sock: socket.socket,
    peer_addr: Tuple[str, int],
    *,
    attempts: int = 20,
    interval: float = 0.2,
) -> bool:
    """Punch toward ``peer_addr``: repeatedly send while listening for the peer's
    packet. Returns True once a packet from the peer is seen (path open)."""
    sock.settimeout(interval)
    ack = PUNCH_MAGIC + b"-ACK"
    for _ in range(max(1, attempts)):
        try:
            sock.sendto(PUNCH_MAGIC, peer_addr)
        except OSError:
            pass
        try:
            data, addr = sock.recvfrom(512)
        except socket.timeout:
            continue
        except OSError:
            continue
        if addr == peer_addr or data.startswith(PUNCH_MAGIC):
            # Send an ACK so the peer (which may still be sending) also confirms.
            try:
                sock.sendto(ack, peer_addr)
            except OSError:
                pass
            return True
    return False


def negotiate_direct_channel(
    channel: Any,
    stun_addr: Tuple[str, int],
    *,
    signal_timeout: float = 5.0,
    punch_attempts: int = 20,
    gather: Callable[..., Tuple[socket.socket, str]] = gather_udp_candidate,
    punch: Callable[..., bool] = hole_punch,
    make_channel: Optional[Callable[[socket.socket, Tuple[str, int]], Any]] = None,
) -> Tuple[Any, bool]:
    """Try to upgrade a paired relay session to a direct, hole-punched channel.

    Both peers call this after their relay session is ready: each gathers a UDP
    candidate from the Edge STUN reflector, exchanges it over the mux
    (``channel.send_signal`` / ``recv_signal``), and punches toward the peer.
    Returns ``(transfer_channel, is_direct)`` -- a reliable-UDP channel when the
    punch succeeds, otherwise the original relay ``channel`` (caller relays).
    """
    try:
        sock, my_candidate = gather(stun_addr)
    except HolePunchUnavailable:
        return channel, False
    try:
        channel.send_signal({"candidate": my_candidate})
        peer = channel.recv_signal(signal_timeout)
        peer_candidate = (peer or {}).get("candidate")
        if not peer_candidate:
            sock.close()
            return channel, False
        peer_addr = parse_addr(peer_candidate)
        if not punch(sock, peer_addr, attempts=punch_attempts):
            sock.close()
            return channel, False
        # Both sides must agree to use the punched path, or one could switch to
        # UDP while the other keeps relaying. Confirm mutually before committing.
        channel.send_signal({"punched": True})
        confirm = channel.recv_signal(signal_timeout)
        if not (confirm or {}).get("punched"):
            sock.close()
            return channel, False
    except Exception:  # noqa: BLE001 -- any failure falls back to relay
        sock.close()
        return channel, False
    factory = make_channel or (lambda s, addr: ReliableUDPChannel.over_socket(s, addr))
    return factory(sock, peer_addr), True
