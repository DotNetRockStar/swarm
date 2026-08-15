# SWARM Protocol Specification (v1)

Three planes. The STUN server coordinates; media never touches it.

| Plane | Transport | Parties | Defined in |
|---|---|---|---|
| Control/REST | HTTPS :443 | browser + devices ↔ STUN | `swarm-core::rest` |
| Signaling/presence | WSS :443 `/api/v1/ws` | devices ↔ STUN | `swarm-core::signal` |
| Media | QUIC over hole-punched UDP | device ↔ device | `swarm-core::peer` |

## Identity & trust

- **User session** — cookie session against the STUN web UI/API (Argon2id, opaque DB-backed, sliding expiry).
- **Device access_token** — opaque 256-bit bearer issued when an 8-digit, single-use, ~15-minute, swarm-scoped join code is redeemed at `POST /api/v1/devices/register`. Stored hashed server-side (revocation = delete row); stored encrypted on-device (OS keychain / Android Keystore).
- **Device certificate** — self-signed, generated on first run, key never leaves the device. Its SHA-256 fingerprint is submitted at registration (the TOFU moment) and re-confirmed inside `signal` offers/answers. Peer authorization = *shares ≥1 swarm AND presented cert's fingerprint matches the pinned roster entry*, enforced at QUIC accept. No CA, no hostname verification.

## Signaling session

1. Device opens WSS, sends `hello {protocol_version, access_token, device_id, capabilities?}`.
2. Server validates and replies `hello_ack {session_id, observed_addr, reflector_ports}`.
3. Server pushes `presence` deltas for devices sharing ≥1 swarm. Server apps fold `streaming {transcode_capacity, active_sessions, hw_accel}` into their presence.
4. `ping`/`pong` keepalive (client-driven, ~30s). Reconnect with capped exponential backoff.
5. `signal {to, payload}` messages are relayed only between devices that share a swarm; the server stamps `from` and never interprets `payload` beyond routing.

## Reflexive address discovery (reflector)

UDP datagram `b"bind"` → JSON reply `{"ip": "<observed ip>", "port": <observed port>}` (≤512 bytes). Reflector listens on UDP/443 (primary) and UDP/3478 (fallback); live ports are advertised in `hello_ack`. Contract is byte-compatible with the retired Batocera.Drone Edge reflector.

## Connection establishment (initiator I → responder R)

1. Both sides gather candidates: `lan` (local interface addrs), `reflexive` (from the reflector, socket kept open), `forwarded` (UPnP/NAT-PMP-mapped or manually forwarded port — server role only).
2. I sends `signal{Offer {punch_id, candidates, cert_fingerprint}}`; R replies `signal{Answer {...}}`.
3. If R has a `forwarded` candidate, I connects directly — no punching.
4. Otherwise simultaneous hole punch: both sides send `PUNCH_MAGIC` (`b"swarm-punch-v1"`) to every remote candidate, LAN first, 20 attempts × 200 ms, while listening on the reflector socket. First received magic → reply and report `signal{Punched {punch_id, ok: true}}`. Both sides proceed only after mutual confirmation (so neither switches while the other still waits).
5. QUIC handshake (TLS 1.3) over the punched 4-tuple. Both sides present their self-signed device certs; each verifies the peer's cert SHA-256 equals the pinned fingerprint. Any mismatch is fatal.
6. On total failure: surface the diagnostics UX (NAT type, UDP reachability, guidance: enable UPnP, port-forward, same LAN). There is **no relay** by design.

Route memory: persist the last successful candidate per peer and try it first next time — preference changes, the trusted candidate set never widens (Drone rule).

## Peer requests (over QUIC)

One request per bidirectional stream. Initiator writes one JSON line (`PeerRequest`), responder writes one JSON line (`PeerResponseHeader`) then `len` raw body bytes.

Routes:

| Path | Body | Notes |
|---|---|---|
| `/catalog/thumbprint` | `CatalogThumbprint` | whole-library version token |
| `/catalog/manifest[?since=<tp>]` | `CatalogManifest` | full or delta listing |
| `/art/{entry_key}/{poster\|backdrop\|cover\|artist}` | image bytes | etag/`if_none_match` honored |
| `/media/{entry_key}` | file bytes | direct play; Range → 206 + `content_range` |
| `/hls/{entry_key}/master.m3u8` | playlist | ladder pruned by the client capability profile |
| `/hls/{entry_key}/{rendition}/{segment}` | MPEG-TS bytes | transcode sessions; `?t=<secs>` on master = seek-into-transcode |

`entry_key` values must pass `entry_key::is_valid_entry_key` before any filesystem lookup. Status vocabulary: 200, 206, 304, 404, 416, 500.

## Catalog identity

- `entry_key = sha256(lowercase(relative_path with / separators))[:24]` — per-server key.
- `fingerprint` = `sample-fp-v1` (MD5 over size:u64-LE + whole file if ≤192 KiB, else head/middle/tail 64 KiB windows) — cross-server identity. Clients merge manifests on fingerprint; multiple servers holding the same fingerprint become alternate sources for one entry.

## Source selection (client, at play time)

Best-first with fall-through on network error, abort on definitive (HTTP-style) error:
online (hard gate) → direct-play-compatible → LAN-tier route → last-successful-route recency → measured signaling RTT → minus transcode saturation. Mid-stream failure re-resolves to the next source at the current position.

## Versioning

`PROTOCOL_VERSION` (currently 1) travels in `hello`; the server rejects mismatches with `error {code: "protocol_version"}` so old apps get a clear update prompt. REST evolution: requests strict (`deny_unknown_fields`), responses extensible.
