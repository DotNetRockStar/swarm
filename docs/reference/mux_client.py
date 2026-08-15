"""Drone-side persistent outbound connection to the Overmind Edge service.

The Drone opens **one** long-lived TLS connection to the Edge and keeps it warm.
This is what lets a Drone work behind a normal home router with no port-forward:
all traffic is Drone-initiated/outbound, and the Edge pushes presence, signaling
and relayed data back down this same connection.

Design split for testability:

* :class:`MuxSession` is the pure, synchronous protocol core for presence /
  keepalive -- feed it decoded frames, it returns frames to send. No threads/I-O.
* :class:`MuxClient` is the threaded runner: it connects a :class:`MuxLink`,
  drives a :class:`MuxSession`, reconnects with capped backoff, and -- for the
  relay transport -- **demultiplexes** the single link into many per-transfer
  :class:`RelayChannel`s. A reader thread reads frames; the main loop dispatches
  them. Both the main loop and worker threads write, so writes are serialized by
  a lock (:meth:`MuxClient._send`).
"""

from __future__ import annotations

import queue
import socket
import ssl
import threading
import time
from typing import Any, Callable, Dict, List, Optional, Protocol
from urllib.parse import urlsplit

from . import mux


class MuxLink(Protocol):
    """A connected duplex byte channel to the Edge."""

    def send(self, data: bytes) -> None: ...

    def read_exactly(self, n: int) -> bytes: ...

    def close(self) -> None: ...


class MuxSession:
    """Pure protocol state machine for one Edge connection (no I/O)."""

    def __init__(
        self,
        *,
        device_id: str,
        token: str,
        capabilities: Optional[List[str]] = None,
        lan_addrs: Optional[List[str]] = None,
        app_version: Optional[str] = None,
        on_presence: Optional[Callable[[list], None]] = None,
        now: Callable[[], float] = time.monotonic,
    ) -> None:
        self.device_id = device_id
        self.token = token
        self.capabilities = list(capabilities or [])
        self.lan_addrs = list(lan_addrs or [])
        self.app_version = app_version
        self._on_presence = on_presence
        self._now = now
        self.session_id: Optional[str] = None
        self.reflexive_addr: Optional[str] = None
        self.connected = False
        self.last_activity = now()

    def build_hello(self) -> bytes:
        message = {
            "type": mux.MSG_HELLO,
            "device_id": self.device_id,
            "token": self.token,
            "capabilities": self.capabilities,
        }
        if self.lan_addrs:
            message["lan_addrs"] = self.lan_addrs
        if self.app_version:
            message["app_version"] = self.app_version
        return mux.encode_control(message)

    def build_ping(self) -> bytes:
        return mux.encode_control({"type": mux.MSG_PING, "t": self._now()})

    def handle_frame(self, kind: int, payload: bytes) -> List[bytes]:
        """Process one received frame; return any frames to send in response.

        Only presence/keepalive control frames are handled here; relay DATA and
        relay control frames are routed by :class:`MuxClient` before this is called.
        """
        self.last_activity = self._now()
        if kind != mux.FRAME_CONTROL:
            return []
        message = mux.decode_control(payload)
        message_type = message.get("type")
        if message_type == mux.MSG_HELLO_ACK:
            self.session_id = message.get("session_id")
            self.reflexive_addr = message.get("reflexive_addr")
            self.connected = True
            return []
        if message_type == mux.MSG_PING:
            return [mux.encode_control({"type": mux.MSG_PONG, "t": message.get("t")})]
        if message_type == mux.MSG_PRESENCE:
            if self._on_presence is not None:
                self._on_presence(message.get("swarm") or message.get("peers") or [])
            return []
        if message_type in (mux.MSG_BYE, mux.MSG_ERROR):
            self.connected = False
        return []


class RelayChannel:
    """A reliable, ordered byte stream for one relayed transfer, tunneled over the
    mux. Implements the PeerChannel interface AssetFetch needs (send /
    read_exactly / close). Reads block in the worker thread until the mux reader
    feeds bytes; writes go out over the shared link via the client's locked send."""

    def __init__(self, session_id: str, send_frame: Callable[[bytes], None]) -> None:
        self._session_id = session_id
        self._send_frame = send_frame
        self._cond = threading.Condition()
        self._buffer = bytearray()
        self._closed = False
        self._failed = False
        self._ready = threading.Event()
        self._signals: "queue.Queue[dict]" = queue.Queue()  # inbound MSG_SIGNAL for this session

    @property
    def session_id(self) -> str:
        return self._session_id

    @property
    def failed(self) -> bool:
        return self._failed

    # --- PeerChannel interface (called from the worker thread) ---
    def send(self, data: bytes) -> None:
        self._send_frame(mux.encode_relay_data(self._session_id, data))

    def read_exactly(self, n: int) -> bytes:
        with self._cond:
            while len(self._buffer) < n and not self._closed:
                self._cond.wait()
            take = min(n, len(self._buffer))
            chunk = bytes(self._buffer[:take])
            del self._buffer[:take]
            return chunk  # fewer than n only at EOF, which read_frame treats as end

    def close(self) -> None:
        try:
            self._send_frame(
                mux.encode_control({"type": mux.MSG_RELAY_CLOSE, "session_id": self._session_id})
            )
        except Exception:  # noqa: BLE001 -- best-effort close notification
            pass
        self.close_remote()

    # --- client/main-loop side ---
    def feed(self, data: bytes) -> None:
        with self._cond:
            self._buffer.extend(data)
            self._cond.notify_all()

    def mark_ready(self) -> None:
        self._ready.set()

    def wait_ready(self, timeout: float) -> bool:
        return self._ready.wait(timeout)

    # --- hole-punch signaling (MSG_SIGNAL over the mux, routed by session) ---
    def send_signal(self, payload: dict) -> None:
        message = {"type": mux.MSG_SIGNAL, "session_id": self._session_id, **payload}
        self._send_frame(mux.encode_control(message))

    def feed_signal(self, message: dict) -> None:
        self._signals.put(message)

    def recv_signal(self, timeout: float) -> Optional[dict]:
        try:
            return self._signals.get(timeout=timeout)
        except queue.Empty:
            return None

    def fail(self) -> None:
        """Mark the session rejected (e.g. Edge sent TRANSFER_ERROR): unblock a
        pending wait_ready and stop reads."""
        with self._cond:
            self._failed = True
            self._closed = True
            self._cond.notify_all()
        self._ready.set()

    def close_remote(self) -> None:
        with self._cond:
            self._closed = True
            self._cond.notify_all()


def parse_edge_endpoint(edge_url: str, default_port: int = 443) -> tuple[str, int]:
    """Parse an Edge endpoint into ``(host, port)``.

    Accepts ``tls://host:port``, ``wss://host:port``, ``https://host:port`` or a
    bare ``host:port`` / ``host``. The scheme only signals "this is a TLS
    endpoint"; the mux framing rides directly on the TLS stream.
    """
    raw = str(edge_url or "").strip()
    if "://" not in raw:
        raw = "tls://" + raw
    parts = urlsplit(raw)
    host = parts.hostname or ""
    if not host:
        raise ValueError(f"invalid edge url: {edge_url!r}")
    return host, int(parts.port or default_port)


class TlsMuxLink:
    """A :class:`MuxLink` backed by a TLS socket."""

    def __init__(self, sock: socket.socket) -> None:
        self._sock = sock
        self._fileobj = sock.makefile("rb")
        self._read_exactly = mux.reader_from_fileobj(self._fileobj)

    def send(self, data: bytes) -> None:
        self._sock.sendall(data)

    def read_exactly(self, n: int) -> bytes:
        return self._read_exactly(n)

    def close(self) -> None:
        for closer in (self._fileobj.close, self._sock.close):
            try:
                closer()
            except OSError:
                pass


def connect_tls(
    edge_url: str,
    *,
    verify: bool = True,
    cafile: Optional[str] = None,
    client_cert: Optional[str] = None,
    client_key: Optional[str] = None,
    timeout: float = 15.0,
    default_port: int = 443,
) -> TlsMuxLink:
    """Open a TLS connection to the Edge and return a :class:`TlsMuxLink`."""
    host, port = parse_edge_endpoint(edge_url, default_port=default_port)
    if verify:
        context = ssl.create_default_context(cafile=cafile)
    else:
        context = ssl._create_unverified_context()  # self-signed Edge / local dev
    if client_cert and client_key:
        try:
            context.load_cert_chain(certfile=client_cert, keyfile=client_key)
        except (ssl.SSLError, OSError):
            # mTLS is best-effort here; HELLO bearer-token auth is authoritative.
            pass
    raw_sock = socket.create_connection((host, port), timeout=timeout)
    try:
        tls_sock = context.wrap_socket(raw_sock, server_hostname=host if verify else None)
    except Exception:
        raw_sock.close()
        raise
    tls_sock.settimeout(None)
    return TlsMuxLink(tls_sock)


class MuxClient:
    """Maintains a single persistent Edge connection in a background thread and
    multiplexes relay transfers over it."""

    def __init__(
        self,
        *,
        connect: Callable[[], MuxLink],
        session_factory: Callable[[], MuxSession],
        ping_interval: float = 20.0,
        idle_timeout_multiplier: float = 3.0,
        backoff_initial: float = 1.0,
        backoff_max: float = 60.0,
        sleep: Callable[[float], None] = time.sleep,
        now: Callable[[], float] = time.monotonic,
        log: Callable[[str], None] = lambda message: None,
        stop_event: Optional[threading.Event] = None,
        on_transfer_offer: Optional[Callable[[dict], None]] = None,
    ) -> None:
        self._connect = connect
        self._session_factory = session_factory
        # Invoked (from the read loop) when the Edge offers a transfer to serve;
        # the handler must not block -- it should hand off to a worker thread.
        self._on_transfer_offer = on_transfer_offer
        self._ping_interval = max(1.0, float(ping_interval))
        self._idle_timeout = self._ping_interval * max(2.0, float(idle_timeout_multiplier))
        self._backoff_initial = backoff_initial
        self._backoff_max = backoff_max
        self._sleep = sleep
        self._now = now
        self._log = log
        self._stop = stop_event or threading.Event()
        self._send_lock = threading.Lock()
        self._link: Optional[MuxLink] = None
        self._sessions: Dict[str, RelayChannel] = {}
        self._sessions_lock = threading.Lock()
        #: Latest session, exposed for status/presence inspection.
        self.session: Optional[MuxSession] = None

    def stop(self) -> None:
        self._stop.set()

    @property
    def connected(self) -> bool:
        return self._link is not None and bool(self.session and self.session.connected)

    def run_forever(self) -> None:
        backoff = self._backoff_initial
        while not self._stop.is_set():
            try:
                connected = self._run_once()
            except Exception as error:  # noqa: BLE001 -- never let the loop die
                self._log(f"edge mux connection error: {error}")
                connected = False
            if self._stop.is_set():
                break
            backoff = self._backoff_initial if connected else min(backoff * 2, self._backoff_max)
            self._sleep(backoff)

    # --- relay session API (called from worker threads) ---
    def start_relay_session(self, session_id: str, role: str) -> RelayChannel:
        """Register a relay leg and send RELAY_OPEN, **without** waiting for
        RELAY_READY (the receiver sends a TRANSFER_REQUEST before waiting)."""
        if role not in ("sender", "receiver"):
            raise ValueError(f"invalid relay role: {role!r}")
        channel = RelayChannel(session_id, self._send)
        with self._sessions_lock:
            self._sessions[session_id] = channel
        try:
            self._send(
                mux.encode_control(
                    {"type": mux.MSG_RELAY_OPEN, "session_id": session_id, "role": role}
                )
            )
        except ConnectionError:
            self._unregister(session_id)
            raise
        return channel

    def open_relay_session(
        self, session_id: str, role: str, *, ready_timeout: float = 20.0
    ) -> RelayChannel:
        """start_relay_session + wait for RELAY_READY. Raises on timeout/rejection."""
        channel = self.start_relay_session(session_id, role)
        if not channel.wait_ready(ready_timeout):
            self._unregister(session_id)
            channel.close_remote()
            raise TimeoutError(f"relay peer not ready for session {session_id}")
        if channel.failed:
            self._unregister(session_id)
            raise ConnectionError(f"relay transfer rejected for session {session_id}")
        return channel

    def send_transfer_request(
        self, session_id: str, token: str, from_device: str, asset: dict
    ) -> None:
        """Ask the Edge to offer this transfer to the sender (receiver side)."""
        self._send(
            mux.encode_control(
                {
                    "type": mux.MSG_TRANSFER_REQUEST,
                    "session_id": session_id,
                    "token": token,
                    "from_device": from_device,
                    "asset": asset,
                }
            )
        )

    def close_relay_session(self, session_id: str) -> None:
        channel = self._unregister(session_id)
        if channel is not None:
            channel.close()

    # --- internals ---
    def _send(self, frame: bytes) -> None:
        with self._send_lock:
            link = self._link
            if link is None:
                raise ConnectionError("mux link not connected")
            link.send(frame)

    def _register(self, session_id: str, channel: RelayChannel) -> None:
        with self._sessions_lock:
            self._sessions[session_id] = channel

    def _unregister(self, session_id: str) -> Optional[RelayChannel]:
        with self._sessions_lock:
            return self._sessions.pop(session_id, None)

    def _get_channel(self, session_id: Optional[str]) -> Optional[RelayChannel]:
        if not session_id:
            return None
        with self._sessions_lock:
            return self._sessions.get(session_id)

    def _fail_all_sessions(self) -> None:
        with self._sessions_lock:
            channels = list(self._sessions.values())
            self._sessions.clear()
        for channel in channels:
            channel.close_remote()

    def _run_once(self) -> bool:
        link = self._connect()
        with self._send_lock:
            self._link = link
        session = self._session_factory()
        self.session = session
        inbound: "queue.Queue[tuple]" = queue.Queue()
        reader = threading.Thread(
            target=self._read_loop, args=(link, inbound), name="edge-mux-reader", daemon=True
        )
        reader.start()
        try:
            self._send(session.build_hello())
            while not self._stop.is_set():
                try:
                    item = inbound.get(timeout=self._ping_interval)
                except queue.Empty:
                    if self._now() - session.last_activity > self._idle_timeout:
                        self._log("edge mux idle timeout; reconnecting")
                        break
                    self._send(session.build_ping())
                    continue
                if item[0] == "error":
                    self._log(f"edge mux read ended: {item[1]}")
                    break
                _, kind, payload = item
                self._handle_inbound(session, kind, payload)
        finally:
            with self._send_lock:
                self._link = None
            self._fail_all_sessions()
            link.close()
            reader.join(timeout=2.0)
        return session.connected

    def _handle_inbound(self, session: MuxSession, kind: int, payload: bytes) -> None:
        # Any inbound frame -- including relay DATA -- is liveness, so a long
        # transfer (only DATA flowing) never trips the idle-timeout reconnect.
        session.last_activity = self._now()
        if kind == mux.FRAME_DATA:
            try:
                session_id, data = mux.parse_relay_data(payload)
            except mux.MuxProtocolError:
                return
            channel = self._get_channel(session_id)
            if channel is not None and data:
                channel.feed(data)
            return
        try:
            message = mux.decode_control(payload)
        except mux.MuxProtocolError:
            return
        message_type = message.get("type")
        if message_type == mux.MSG_RELAY_READY:
            channel = self._get_channel(message.get("session_id"))
            if channel is not None:
                channel.mark_ready()
            return
        if message_type == mux.MSG_RELAY_CLOSE:
            channel = self._unregister(message.get("session_id"))
            if channel is not None:
                channel.close_remote()
            return
        if message_type == mux.MSG_TRANSFER_OFFER:
            if self._on_transfer_offer is not None:
                self._on_transfer_offer(message)
            return
        if message_type == mux.MSG_TRANSFER_ERROR:
            channel = self._get_channel(message.get("session_id"))
            if channel is not None:
                channel.fail()
            return
        if message_type == mux.MSG_SIGNAL:
            channel = self._get_channel(message.get("session_id"))
            if channel is not None:
                channel.feed_signal(message)
            return
        # Presence / keepalive: delegate to the pure session core.
        for frame in session.handle_frame(kind, payload):
            self._send(frame)

    @staticmethod
    def _read_loop(link: MuxLink, inbound: "queue.Queue[tuple]") -> None:
        try:
            while True:
                kind, payload = mux.read_frame(link.read_exactly)
                inbound.put(("msg", kind, payload))
        except Exception as error:  # noqa: BLE001 -- propagate as a queue item
            inbound.put(("error", error))
