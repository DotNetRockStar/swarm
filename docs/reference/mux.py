"""Mux wire protocol: framing + message types for the Drone<->Edge link.

The Drone keeps a single persistent **outbound** TLS connection to the Overmind
Edge service (so a Drone never has to accept inbound connections / be
port-forwarded). That one connection is a *mux*: it multiplexes presence,
heartbeat, transfer signaling, and -- for relayed transfers -- chunked file data.

This module is the pure, stdlib-only codec shared by everything that speaks the
protocol. It has no sockets and no I/O of its own so it is trivially testable;
:mod:`app.transport.mux_client` layers the actual TLS connection on top. The
Edge server (in the Overmind repo) implements a byte-compatible encoder/decoder.

Wire format -- every frame is::

    +----------+------------------------+========================+
    | kind     | length (uint32, BE)    | payload (length bytes) |
    | (1 byte) | big-endian             |                        |
    +----------+------------------------+========================+

``kind`` is one of the ``FRAME_*`` constants. ``length`` is the payload size in
bytes (capped by :data:`MAX_FRAME_PAYLOAD`). CONTROL payloads are a UTF-8 JSON
object carrying a ``"type"`` field (one of the ``MSG_*`` constants). DATA frames
(used by the relay transport, Phase 2) carry binary chunk data; their payload
layout is defined where the relay is implemented.
"""

from __future__ import annotations

import json
import struct
from typing import Any, Callable, Mapping, Tuple

# Frame kinds (first header byte).
FRAME_CONTROL = 0x01  # JSON control message
FRAME_DATA = 0x02  # binary payload (relay chunk data)

#: Hard cap on a single frame payload so a malformed/hostile length can never
#: make a peer attempt a huge allocation. 16 MiB comfortably exceeds any control
#: message and any sane relay chunk.
MAX_FRAME_PAYLOAD = 16 * 1024 * 1024

_HEADER = struct.Struct(">BI")  # kind (uint8), length (uint32 big-endian)
HEADER_SIZE = _HEADER.size

# Control message ``type`` values. Phase 1 uses the presence/keepalive subset;
# later phases add ACTION / SIGNAL / RELAY_* (kept here as the single source of
# truth for the message vocabulary).
MSG_HELLO = "hello"  # Drone -> Edge: authenticate + register capabilities
MSG_HELLO_ACK = "hello_ack"  # Edge -> Drone: session id + reflexive address
MSG_PING = "ping"  # either direction: keepalive request
MSG_PONG = "pong"  # either direction: keepalive reply
MSG_PRESENCE = "presence"  # Edge -> Drone: swarm presence deltas
MSG_BYE = "bye"  # either direction: graceful close
MSG_ERROR = "error"  # either direction: protocol/auth error before close
MSG_RELAY_OPEN = "relay_open"  # Drone -> Edge: join a transfer session as a role
MSG_RELAY_READY = "relay_ready"  # Edge -> Drone: both legs present, data may flow
MSG_RELAY_CLOSE = "relay_close"  # either direction: tear down a transfer session
MSG_TRANSFER_REQUEST = "transfer_request"  # receiver -> Edge: ask to pull an asset
MSG_TRANSFER_OFFER = "transfer_offer"  # Edge -> sender: serve this asset
MSG_TRANSFER_ERROR = "transfer_error"  # Edge -> receiver: offer could not be set up
MSG_SIGNAL = "signal"  # Drone <-> Edge <-> Drone: hole-punch candidate exchange

#: A relay DATA frame's payload is a fixed-width transfer session id (uuid4 hex)
#: followed by the chunk bytes, so the Edge can route it to the paired leg.
RELAY_SESSION_ID_LEN = 32


class MuxProtocolError(Exception):
    """Raised on a malformed frame, oversized payload, or bad control JSON."""


def encode_frame(kind: int, payload: bytes) -> bytes:
    """Encode a single frame (header + payload)."""
    if kind < 0 or kind > 0xFF:
        raise MuxProtocolError(f"invalid frame kind: {kind}")
    if len(payload) > MAX_FRAME_PAYLOAD:
        raise MuxProtocolError(
            f"frame payload too large: {len(payload)} > {MAX_FRAME_PAYLOAD}"
        )
    return _HEADER.pack(kind, len(payload)) + payload


def encode_control(message: Mapping[str, Any]) -> bytes:
    """Encode a CONTROL frame from a JSON-serializable mapping.

    The mapping must carry a ``"type"`` key (one of the ``MSG_*`` constants); the
    encoder does not invent one so callers can't accidentally send a typeless
    control frame.
    """
    if "type" not in message:
        raise MuxProtocolError("control message requires a 'type' field")
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    return encode_frame(FRAME_CONTROL, payload)


def decode_control(payload: bytes) -> dict:
    """Decode a CONTROL frame payload into a dict (must be a JSON object)."""
    try:
        message = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as error:
        raise MuxProtocolError(f"invalid control JSON: {error}") from error
    if not isinstance(message, dict):
        raise MuxProtocolError("control message must be a JSON object")
    return message


def read_frame(read_exactly: Callable[[int], bytes]) -> Tuple[int, bytes]:
    """Read one frame using a ``read_exactly(n) -> bytes`` callable.

    ``read_exactly`` must return exactly ``n`` bytes or raise/return ``b""`` at
    EOF; this keeps the codec independent of sockets (tests pass a BytesIO-backed
    reader, the client passes a socket-backed one). Returns ``(kind, payload)``.
    Raises :class:`MuxProtocolError` on a truncated stream or oversized frame,
    and :class:`EOFError` when the stream ends cleanly at a frame boundary.
    """
    header = read_exactly(HEADER_SIZE)
    if not header:
        raise EOFError("connection closed at frame boundary")
    if len(header) != HEADER_SIZE:
        raise MuxProtocolError("truncated frame header")
    kind, length = _HEADER.unpack(header)
    if length > MAX_FRAME_PAYLOAD:
        raise MuxProtocolError(f"declared frame payload too large: {length}")
    if length == 0:
        return kind, b""
    payload = read_exactly(length)
    if len(payload) != length:
        raise MuxProtocolError("truncated frame payload")
    return kind, payload


def encode_relay_data(session_id: str, data: bytes) -> bytes:
    """Encode a relay DATA frame: ``[FRAME_DATA][len][session_id(32) + data]``."""
    sid = str(session_id).encode("ascii")
    if len(sid) != RELAY_SESSION_ID_LEN:
        raise MuxProtocolError(f"relay session id must be {RELAY_SESSION_ID_LEN} chars")
    return encode_frame(FRAME_DATA, sid + data)


def parse_relay_data(payload: bytes) -> Tuple[str, bytes]:
    """Split a relay DATA frame payload into ``(session_id, data)``."""
    if len(payload) < RELAY_SESSION_ID_LEN:
        raise MuxProtocolError("relay data frame shorter than session id")
    return payload[:RELAY_SESSION_ID_LEN].decode("ascii", "replace"), payload[RELAY_SESSION_ID_LEN:]


def reader_from_fileobj(fileobj: Any) -> Callable[[int], bytes]:
    """Adapt a binary file-like object (e.g. ``socket.makefile('rb')``) into a
    ``read_exactly`` callable suitable for :func:`read_frame`."""

    def read_exactly(n: int) -> bytes:
        chunks = []
        remaining = n
        while remaining > 0:
            chunk = fileobj.read(remaining)
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        return b"".join(chunks)

    return read_exactly
