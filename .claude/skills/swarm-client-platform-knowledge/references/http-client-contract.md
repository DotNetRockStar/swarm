# The client↔server contract

Transport-agnostic. Fire TV reaches most of this over QUIC (`PeerRequest`/`PeerResponseHeader`
JSON-over-stream, `clients/tv-android/core/.../peer/Contracts.kt`) via its loopback proxy;
Roku and any future HTTP-only client reach the **identical** path/payload shape over plain
HTTP(S) through `apps/server/src/http_media.rs`, whose whole reason to exist is "same
protocol, different envelope" (`resolve_for_client` and the QUIC dispatch's
`resolve_for_transport` both funnel into the same `MediaService`). Read this file, then
read whichever of the two transport docs matches your platform's capability
(`crates/swarm-p2p` for QUIC, `apps/server/src/http_media.rs` for HTTP) — don't re-derive
either from scratch.

Everything below is keyed by **path**, not by transport. `{base}` = the loopback proxy URL
prefix on Fire TV, or `https://{server}:8547` / relay host on Roku.

## Rendezvous (STUN) REST — `apps/stun-server`, always plain HTTPS + JSON

| Method | Path | Auth | Purpose |
|---|---|---|---|
| POST | `/api/v1/devices/register` | none | First-time device registration with a join code → `access_token` |
| POST | `/api/v1/activations` | optional bearer | Start an activation (device shows a code, owner approves) |
| GET | `/api/v1/activations/{id}` | bearer = poll token | Poll activation status: `pending` / `approved` / `expired` |
| POST | `/api/v1/swarms/join` | bearer = access token | Join an additional swarm by 8-char code |
| GET | `/api/v1/swarms/{id}/devices` | bearer | Roster: every device in the swarm, with `metadata` |
| DELETE | `/api/v1/swarms/{id}/devices/{deviceId}` | bearer | Leave a swarm |
| PATCH | `/api/v1/devices/{id}/metadata` | bearer | Publish routing metadata (`peer_addr`, `relay_addr`, …) |

`SwarmDevice.metadata` is a free-form string map — this is how a server publishes its
reachability (QUIC `peer_addr`, or a Roku-relevant `http_media_port`/`relay_addr` once
built) without a schema change. A client should read metadata defensively (missing keys are
normal) and never assume every key a Fire TV expects is present for every device.

**401 on any bearer call ⇒ drop the stored token and re-prompt for pairing.** Every client
must treat this the same way — a stale/revoked token is not a retryable error.

## Signaling (WSS) — QUIC-capable clients only

`wss://{stun-host}/api/v1/ws`, one JSON text frame per message, tagged on `"type"`
(`hello`/`hello_ack`/`ping`/`pong`/`presence`/`signal`/`bye`/`error`). An HTTP-only client
(Roku) has **no equivalent** — it never needs a live signaling session because it never
hole-punches. Skip this section entirely for such a platform; it is not a gap to fill in,
it's out of scope by construction.

## Media-server surface — identical path space on both transports

Base examples: Fire TV's loopback proxy (`http://127.0.0.1:{port}/{serverId}{path}`) or
Roku's direct HTTPS (`https://{host}:8547{path}` with `Authorization: Bearer {token}`).

| Method | Path | Notes |
|---|---|---|
| GET | `/catalog/thumbprint` | `{thumbprint, entry_count}` — cheap "has anything changed" check |
| GET | `/catalog/manifest` (or `.gz`) | Full `CatalogManifest` — fall back to non-gz on 404 |
| GET | `/catalog/changes?since={thumbprint}` (or `.gz`) | Long-poll delta: 200 = delta, **204 = quiet timeout**, 404 = unsupported, keep polling with a floor delay |
| POST | `/play/{entry_key}` | Body = `PlaybackPreferences` → `PlaybackPlan` JSON. **The** negotiation call. |
| GET | `/stream/{session_id}/media` | Direct-play bytes, full `Range`/206/`Content-Range` support |
| GET | `/media/{entry_key}` | Direct bytes without a live session (previews, etc.) |
| GET | `/hls/{session_id}/{*rest}` | HLS master + variant playlists + segments — must be a catch-all, nested paths are real (`v0/index.m3u8`, `v0/init.mp4`, `v0/segment_000123.m4s`) |
| GET | `/art/{entry_key}/{kind}?w={px}` | `kind` ∈ poster/cover/season/backdrop/artist. `If-None-Match` → 304. No width param = full-res. |
| POST | `/stop/{session_id}` | **Must be called on every playback teardown path** — see state-model.md's session discipline |
| GET | `/subtitles/{entry_key}/{filename}` | WebVTT |
| POST | `/errors/report` | Body = `ClientErrorReport` → 204. Queue locally on failure, cap the queue, flush on next successful catalog connect. |
| GET | `/notifications/{device_id}` | Resolved problem-report notifications for this device |
| POST | `/notifications/{device_id}/{id}/dismiss` | Best-effort, fire-and-forget |
| POST | `/likes/toggle` | Body = `LikeToggle{device_id, device_name, entry_key, liked}` — idempotent, desired end-state not a toggle-in-place |

### `PlaybackPreferences` → `PlaybackPlan`

Request:
```json
{
  "capabilities": { "containers": [...], "video_codecs": [...], "audio_codecs": [...],
                     "max_width": N, "max_height": N, "max_bitrate": N, "hdr": bool },
  "start_position_secs": 0,
  "prefer_direct": true,
  "preview": false
}
```
`capabilities.containers` **must include `"hls"`** or the server returns 503 `Unsupported`
— a client that can only play HLS still has to advertise it wants HLS as a fallback
container even if it also wants direct play.

Response, `mode` decides everything downstream:
```json
{ "mode": "direct" | "hls", "path": "/stream/{id}/media" | "/hls/{id}/master.m3u8",
  "max_bitrate": N, "session_id": "...",
  "lyrics": {...}?, "subtitles": [{id, language, label, source, path}] }
```
- `mode = "direct"`: `resume_position_secs` goes into `start_position_secs` on the request
  (server seeks the file); the client plays from position 0 in its own player timeline
  and adds the resume offset back only for the "already watched N%" UI math.
- `mode = "hls"`: the server always starts the HLS stream at `start_position_secs`, so the
  client must track a **position offset** = the resume position, treat the player's own
  timeline as starting at 0, and add the offset back everywhere it reports/displays
  position. Getting this backwards double-applies or drops the resume offset — reproduce
  Fire TV's `resumePositionSecs`/`positionOffsetSecs` split exactly (see state-model.md).

**HLS output format** (`crates/swarm-media/src/transcode.rs`): fMP4/CMAF segments
(`.m4s` + `init.mp4`), **not MPEG-TS**. `#EXT-X-PLAYLIST-TYPE:EVENT` — the playlist grows
while a live transcode is in progress and gets `#EXT-X-ENDLIST` appended when done; a
client's player must tolerate a growing event playlist, not assume VOD-style completeness
on first fetch. Startup (cold transcode) can take up to ~120s server-side — size any
client-side negotiation timeout comfortably above that (Fire TV uses 135s).

**Session discipline is not optional.** A `session_id` reserves server-side transcode
capacity/bandwidth. Exactly one negotiation in flight at a time per client; every
abandoned/superseded/torn-down session gets an explicit `POST /stop/{session_id}` rather
than left to idle-expire — a client that skips this will eventually see 429
`Bandwidth`/`Capacity` from its own leaked reservations. HTTP-only clients get **no**
cross-cancel protection the way QUIC peers do (`resolve_for_client` passes
`playback_owner = None`) — be extra disciplined about explicit stop calls on that path.

### Retry policy (reproduce exactly — this is tuned against real flaky-Wi-Fi/cold-start behavior)

| Operation | Policy |
|---|---|
| Catalog refresh per server | 3 attempts, ~500ms gap before attempts 2 & 3 |
| `preparePlayback` (`/play`) | 1 retry after evicting/reconnecting the transport |
| `/stop` | 1 retry after eviction |
| `/errors/report` | 2 attempts + eviction, then queue locally (cap ~20, drop oldest) |
| Activation / pairing status poll | every 2s, unbounded until expiry |
| Catalog change-feed long-poll | continuous, ~250ms floor delay between polls |
| Dashboard presence refresh | every 10s |
| Catalog load timeout (whole sequence) | 45s, then report a per-server client error and show a stale/cached result rather than blocking forever |
| Foreground playback negotiation timeout | 135s (server startup cap is ~120s) |
| Preview playback negotiation timeout | 15s (server preview cap is ~10s) |

### Error status → meaning (both transports)

- **429** → `Capacity`/`Bandwidth` exhaustion — not a bug, back off, don't hammer
- **400** → `MissingPreferences` or similar client-side mistake
- **404 on a direct-play stream mid-playback** → treat as an **expired session**, not a
  generic error: renegotiate a fresh session at the current absolute position rather than
  surfacing a hard error to the user (server sessions expire after several idle minutes)
- **anything else 4xx/5xx** → surface as a real error, report via `/errors/report`

## Capability profile

Every client must build a `CapabilityProfile` and merge it with a platform baseline using
**union, never subtraction** semantics: a failed probe degrades to exactly the baseline, it
never removes something a successful probe already established could work. See
state-model.md for the exact merge algorithm (ported field-for-field from
`CapabilityMapping.kt`, unit-tested there — reuse the same test cases for a new platform's
port). `containers` must always include `"hls"`. Clamp advertised resolution to the panel's
actual resolution — never advertise more than the screen can show.

## Artwork caching contract (client-side, but the rules are protocol-shaped)

- Cache key = server-id-scoped path + query, **excluding** any transient transport-routing
  detail (a loopback proxy port, a relay session token) — only `entry_key`, `kind`, `?v=`
  (the artwork ETag) and `?w=` (requested width) are part of the identity. Getting this
  wrong either cache-busts on every restart or serves stale art forever.
- TTL = 30 days by default (`AndroidAppSettingsStore.DEFAULT_ARTWORK_CACHE_MINUTES`), and
  the timestamp is only refreshed on a **successful** re-fetch — a cache hit must not slide
  the expiration, and a failed refresh must stay retryable next time.
- `If-None-Match` round-trips to the server's `ETag`; a 304 means "what you cached is still
  correct," not "fetch failed."

## What deliberately has no HTTP equivalent

`/pair/begin` and `/pair/poll` (the HTTP-only pairing flow in `http_media.rs`) have **no**
QUIC equivalent — a QUIC-capable peer authenticates by presenting a cert at the transport
layer and never needs to "pair" over the wire the way a credential-less HTTP device does.
Conversely, Fire TV's raw-NDJSON LAN pairing protocol (`lan.rs`, `pairing_port`) is a
**different, older** protocol that an HTTP-only client should never implement — use
`/pair/begin`/`/pair/poll` instead, discovered via the mDNS TXT key `http_media_port` (or
its TLS counterpart once shipped), not `pair_port`.
