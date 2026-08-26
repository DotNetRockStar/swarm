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

### STUN-free LAN discovery and pairing

Media servers also advertise `_swarm-peer._udp.local.` over mDNS with their QUIC port, certificate fingerprint, and local pairing port. Android clients browse that service with DNS-SD and construct the same pinned `SwarmDevice` shape used by a STUN roster; catalog and playback traffic therefore continue over the existing mutually authenticated QUIC transport and never pass through the pairing socket.

Discovery is not authorization. For a first connection, the server owner explicitly opens a five-minute, single-use pairing window in the desktop app and enters its six-digit code on the client. The client submits its name and certificate fingerprint from a private/link-local source address. The server persists that fingerprint in `server-state.sqlite` and adds it to `AllowedPeers`. Later launches can connect directly to the rediscovered server without STUN or another code. Revoking the local peer removes it from both persistent storage and the live allow-list.

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
| `/catalog/manifest.gz` | gzip-compressed `CatalogManifest` | bandwidth-efficient full listing for memory-constrained clients |
| `/art/{entry_key}/{poster\|season\|backdrop\|cover\|artist}` | image bytes | etag/`if_none_match` honored |
| `/play/{entry_key}` | `PlaybackPlan` | request carries `PlaybackPreferences`; reserves upload and chooses direct/HLS |
| `/stream/{session_id}/media` | file bytes | budgeted direct play; Range → 206 + `content_range` |
| `/media/{entry_key}` | file bytes | legacy/unnegotiated direct play; globally upload-paced |
| `/hls/{session_id}/master.m3u8` | multivariant playlist | ladder pruned by client limits and remaining server upload |
| `/hls/{session_id}/{rendition}/{file}` | playlist/fMP4 bytes | four-second CMAF/fMP4 HLS; resume offset was supplied during `/play` |
| `/errors/report` | empty (204) | request carries a client problem report for server-side triage |
| `/notifications/{device_id}` | `ClientResolutionNotification[]` | resolved reports for that client that have not been dismissed |
| `/notifications/{device_id}/{error_id}/dismiss` | empty (204) | acknowledges and dismisses one resolved-problem notification |
| `/likes/toggle` | empty (204) | request carries an idempotent desired like state |

`entry_key` values must pass `entry_key::is_valid_entry_key` before any filesystem lookup. Session ids are random 128-bit values. Status vocabulary additionally uses 400 for missing playback preferences, 429 for exhausted session/upload capacity, and 503 for an unavailable/failed transcoder.

### Playback negotiation and upload budget

`PeerRequest.playback`, present only on `/play/*`, contains the client's
`CapabilityProfile`, integer resume position, and direct-play preference. A
successful `PlaybackPlan` returns `mode`, a session-scoped peer `path`, and
the session's hard `max_bitrate`. Capabilities travel directly over pinned
QUIC; the rendezvous service is not trusted with the playback decision.

The server computes `usable_upload = max_upload × (100 - reserve_percent) / 100`.
Every direct or HLS session reserves a conservative peak rate, and the sum of
live reservations may not exceed that shared pool. Response bodies are paced
to the reservation rather than bursting onto the uplink. Default ladder:

| Name | Maximum dimensions | Average video | Peak video | Stereo AAC |
|---|---:|---:|---:|---:|
| 1080p | 1920×1080 | 6 Mbps | 8 Mbps | 192 kbps |
| 720p | 1280×720 | 3 Mbps | 4 Mbps | 160 kbps |
| 480p | 854×480 | 1.4 Mbps | 2 Mbps | 128 kbps |
| 360p | 640×360 | 700 kbps | 1 Mbps | 96 kbps |

Rungs above the source, client limits, or remaining upload are omitted. Video
is H.264 High/4.1, yuv420p with aligned two-second keyframes; media segments
target four seconds. Music direct-plays when compatible and otherwise uses a
single 96–192 kbps stereo AAC HLS rendition. Idle sessions and their temporary
segments expire after five minutes; active file transfers cannot expire.

H.264 High/4.1 + AAC and fMP4/CMAF HLS were chosen specifically because every
target client (Fire TV's `MediaCodec`, ExoPlayer's HLS demuxer) hardware- or
natively-decodes them with no additional negotiation beyond this
`CapabilityProfile`/`PlaybackPlan` exchange — there is no separate
lower-bandwidth/faster-decode codec question left open. Validated end-to-end
on real Fire TV hardware: direct play and forced-HLS transcode both produce
correct, low-latency playback.

## Catalog identity

- `entry_key = sha256(lowercase(relative_path with / separators))[:24]` — per-server key.
- `fingerprint` = `sample-fp-v1` (MD5 over size:u64-LE + whole file if ≤192 KiB, else head/middle/tail 64 KiB windows) — cross-server identity. Clients merge manifests on fingerprint; multiple servers holding the same fingerprint become alternate sources for one entry.

## Source selection (client, at play time)

Best-first with fall-through on network error, abort on definitive (HTTP-style) error:
online (hard gate) → direct-play-compatible → LAN-tier route → last-successful-route recency → measured signaling RTT → minus transcode saturation. Mid-stream failure re-resolves to the next source at the current position.

## Versioning

`PROTOCOL_VERSION` (currently 1) travels in `hello`; the server rejects mismatches with `error {code: "protocol_version"}` so old apps get a clear update prompt. REST evolution: requests strict (`deny_unknown_fields`), responses extensible.
