"""Reliable, ordered byte channel over a (hole-punched) UDP socket.

AssetFetch needs a reliable ordered stream; UDP gives neither. This is a small
sliding-window ARQ (TCP-like):

* the **sender** keeps a window of unacknowledged DATA packets and retransmits
  ones still unacked after an RTO;
* the **receiver** buffers out-of-order packets, delivers bytes in order, and
  cumulatively ACKs the next sequence it still needs.

It implements the PeerChannel interface (``send`` / ``read_exactly`` / ``close``)
so the existing AssetFetch download/serve runs over a punched path unchanged. A
background I/O thread does receive + ACK processing + retransmit; the caller
thread does ``send`` / ``read_exactly``.

Fixed window, no congestion control -- adequate for moderate-RTT links; QUIC
would be the upgrade. Correctness (no loss/reorder corruption) is the priority.
"""

from __future__ import annotations

import struct
import threading
import time
from collections import OrderedDict
from typing import Callable, Dict, Optional, Tuple

_DATA = 0x01
_ACK = 0x02  # value = next sequence the receiver still needs (cumulative)
_FIN = 0x03  # value = total DATA packets the sender produced
_HEADER = struct.Struct(">BI")

DEFAULT_MTU_PAYLOAD = 1100  # conservative UDP payload to avoid IP fragmentation


class ReliableUDPChannel:
    def __init__(
        self,
        *,
        send_datagram: Callable[[bytes], None],
        recv_datagram: Callable[[float], Optional[bytes]],
        mtu_payload: int = DEFAULT_MTU_PAYLOAD,
        window: int = 32,
        rto: float = 0.3,
        tick: float = 0.05,
        now: Callable[[], float] = time.monotonic,
    ) -> None:
        self._send_dg = send_datagram
        self._recv_dg = recv_datagram
        self._mtu = max(1, int(mtu_payload))
        self._window = max(1, int(window))
        self._rto = rto
        self._tick = tick
        self._now = now

        # Send side (guarded by _lock; _window_cv signals window room / drain).
        self._lock = threading.Lock()
        self._window_cv = threading.Condition(self._lock)
        self._next_seq = 0
        self._unacked: "OrderedDict[int, Tuple[bytes, float]]" = OrderedDict()

        # Receive side (guarded by _recv_cv).
        self._recv_cv = threading.Condition()
        self._expected = 0
        self._reorder: Dict[int, bytes] = {}
        self._inorder = bytearray()
        self._peer_fin_seq: Optional[int] = None
        self._eof = False

        self._closed = threading.Event()
        self._error: Optional[BaseException] = None
        self._io = threading.Thread(target=self._io_loop, name="reliable-udp-io", daemon=True)
        self._io.start()

    # --- PeerChannel interface (caller thread) ---
    def send(self, data: bytes) -> None:
        view = memoryview(data)
        for start in range(0, len(view), self._mtu):
            self._send_chunk(bytes(view[start : start + self._mtu]))

    def read_exactly(self, n: int) -> bytes:
        with self._recv_cv:
            # Block until n bytes are available in order, or the stream ends.
            while len(self._inorder) < n and not self._eof and not self._closed.is_set():
                self._recv_cv.wait(timeout=self._tick)
            take = min(n, len(self._inorder))
            out = bytes(self._inorder[:take])
            del self._inorder[:take]
            return out

    def close(self) -> None:
        # Best-effort flush: let outstanding DATA drain before we stop.
        deadline = self._now() + 2.0
        with self._window_cv:
            while self._unacked and not self._closed.is_set() and self._now() < deadline:
                self._window_cv.wait(timeout=self._tick)
        for _ in range(3):
            try:
                self._send_dg(_HEADER.pack(_FIN, self._next_seq))
            except OSError:
                break
        self._closed.set()
        self._wake_all()
        self._io.join(timeout=2.0)

    # --- send helpers ---
    def _send_chunk(self, chunk: bytes) -> None:
        with self._window_cv:
            while not self._closed.is_set() and len(self._unacked) >= self._window:
                self._window_cv.wait(timeout=self._tick)
            if self._closed.is_set():
                raise self._error or ConnectionError("reliable udp channel closed")
            seq = self._next_seq
            self._next_seq += 1
            self._unacked[seq] = (chunk, self._now())
        self._send_dg(_HEADER.pack(_DATA, seq) + chunk)

    # --- I/O thread ---
    def _io_loop(self) -> None:
        try:
            while not self._closed.is_set():
                self._retransmit_due()
                data = self._recv_dg(self._tick)
                if data:
                    self._handle(data)
        except Exception as error:  # noqa: BLE001 -- surface to callers, don't crash
            self._fail(error)

    def _retransmit_due(self) -> None:
        now = self._now()
        with self._lock:
            due = [
                (seq, chunk)
                for seq, (chunk, sent) in self._unacked.items()
                if now - sent >= self._rto
            ]
            for seq, chunk in due:
                self._unacked[seq] = (chunk, now)
        for seq, chunk in due:
            self._send_dg(_HEADER.pack(_DATA, seq) + chunk)

    def _handle(self, data: bytes) -> None:
        if len(data) < _HEADER.size:
            return
        kind, value = _HEADER.unpack(data[: _HEADER.size])
        payload = data[_HEADER.size :]
        if kind == _ACK:
            self._on_ack(value)
        elif kind == _DATA:
            self._on_data(value, payload)
        elif kind == _FIN:
            self._on_fin(value)

    def _on_ack(self, ack: int) -> None:
        with self._window_cv:
            for seq in [s for s in self._unacked if s < ack]:
                del self._unacked[seq]
            self._window_cv.notify_all()

    def _on_data(self, seq: int, payload: bytes) -> None:
        with self._recv_cv:
            if seq == self._expected:
                self._inorder.extend(payload)
                self._expected += 1
                while self._expected in self._reorder:
                    self._inorder.extend(self._reorder.pop(self._expected))
                    self._expected += 1
                self._refresh_eof()
                self._recv_cv.notify_all()
            elif seq > self._expected and seq < self._expected + self._window * 2:
                self._reorder.setdefault(seq, payload)
            # seq < expected: duplicate; just re-ack below.
        self._send_dg(_HEADER.pack(_ACK, self._expected))

    def _on_fin(self, final_seq: int) -> None:
        with self._recv_cv:
            self._peer_fin_seq = final_seq
            self._refresh_eof()
            self._recv_cv.notify_all()
        self._send_dg(_HEADER.pack(_ACK, self._expected))

    def _refresh_eof(self) -> None:
        if (
            self._peer_fin_seq is not None
            and self._expected >= self._peer_fin_seq
            and not self._reorder
        ):
            self._eof = True

    def _fail(self, error: BaseException) -> None:
        self._error = error
        self._closed.set()
        self._wake_all()

    def _wake_all(self) -> None:
        with self._recv_cv:
            self._recv_cv.notify_all()
        with self._window_cv:
            self._window_cv.notify_all()

    # --- construction over a real UDP socket ---
    @classmethod
    def over_socket(cls, sock, peer_addr: Tuple[str, int], **kwargs) -> "ReliableUDPChannel":
        def send_dg(data: bytes) -> None:
            sock.sendto(data, peer_addr)

        def recv_dg(timeout: float) -> Optional[bytes]:
            sock.settimeout(timeout)
            try:
                data, addr = sock.recvfrom(65535)
            except (OSError, ValueError):
                return None
            return data if addr == peer_addr else None

        return cls(send_datagram=send_dg, recv_datagram=recv_dg, **kwargs)
