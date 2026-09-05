# Platform notes — Roku

`clients/tv-roku`. BrighterScript (`.bs`, compiles to `.brs`) + SceneGraph XML, built with
`bsc`/`ropm`/`roku-deploy`, unit-tested with `rooibos`. No QUIC, no custom transports — the
whole client speaks plain HTTPS to `apps/server/src/http_media.rs` (see
`http-client-contract.md`) plus the STUN REST API.

## Portable-concept ↔ platform-primitive map

| Portable concept | Roku primitive |
|---|---|
| Observable app state | One SceneGraph node (`AppState.xml`/`.bs`) with fields per piece of state; screens `observeField`/`observeFieldScoped` |
| Async I/O | Task nodes (`HttpTask`, `CatalogTask`, `PlaybackTask`, `PairingTask`, `MdnsDiscoveryTask`, `ArtworkTask`, `ChangeFeedTask`) — the render thread must never block |
| Screen focus/D-pad | `setFocus(true)`, `focusable` fields, `onKeyEvent()` handlers |
| Durable relational storage | **None available** — `roRegistrySection`, ~32KB/section cap, flat string KV only. See "Storage" below. |
| Durable flat KV storage | `roRegistrySection`, JSON-encoded blobs per key, LRU-capped where unbounded on Fire TV |
| Secrets | Same registry (Roku's registry is not readable off-device by another app or a file browser the way a rooted Android's filesystem is) |
| Device identity | No persistent hardware cert equivalent — see "Identity & pairing" below |
| Catalog/artwork cache | `cachefs:/` (OS-evictable, do not rely on it surviving) |
| Image loading | `Poster` node from a **local file path**, never a remote URL directly (no HTTP header support — see "Artwork" below) |
| Video/audio playback | `Video` node + `ContentNode` |
| Transport (remote) | Plain HTTPS to a TLS-passthrough relay (server-side work, see the server-side skill/plan for issue #54) |
| Transport (LAN) | Plain HTTP/HTTPS, address from hand-rolled mDNS over `roDatagramSocket` |
| Media→player bridge | **None needed** — `Video` node consumes the server's HTTP(S) URLs directly |
| Capability probe | `roDeviceInfo.CanDecodeVideo()`/`GetVideoMode()`/`GetDisplayProperties()` |

## Hard platform constraints (read before designing anything)

### Storage: the registry is small, flat, and per-channel

`roRegistrySection` sections are capped at roughly 32KB each. There is no relational store,
no query language, no migrations framework. Consequences for the port:
- Split state into multiple sections by concern (`swarm.auth`, `swarm.conn`,
  `swarm.settings`, `swarm.watch`, `swarm.lists`, `swarm.notify`) rather than one big blob —
  smaller independent budgets are easier to reason about and a corrupt/oversized one section
  doesn't take down every other kind of state.
- Anything **unbounded on Fire TV** (watch state keyed by fingerprint, likes, watchlist,
  resolved-notification tombstones) needs an explicit LRU cap on Roku — this is a real,
  intentional platform deviation, not an oversight. Record the cap value once chosen in the
  feature-inventory row for "on-device state persistence."
- Guard every registry write with a size check (`GetSize()`) before `Flush()` — silently
  failing to persist state is worse than refusing an update and logging it.
- Catalog manifest and artwork stay in `cachefs:/`, not the registry — they're
  reconstructable from the server, so OS eviction under memory pressure is an acceptable
  (if annoying) failure mode; a lost auth token or watch-state row is not.

### Artwork: `Poster` can't send an `Authorization` header

`/art/*` requires a bearer token, but SceneGraph's `Poster` node takes a bare `uri` with no
header support. A client can't point `Poster` at the server directly the way it can for a
public/unauthenticated image host. The fix (mirrors, doesn't reinvent, `ArtworkCache.kt`'s
semantics): a bounded pool of `ArtworkTask`s fetches via `roUrlTransfer` (which *does*
support `AddHeader`), writes the bytes to `cachefs:/artwork/<cache-key>`, and only then
hands `Poster` the local file path. Reuse the exact same cache-key shape and TTL/refresh
rules as `http-client-contract.md`'s artwork section — this is protocol-shaped state, not
Roku-specific.

If real-hardware measurement later shows this pool can't keep a 5-wide grid populated fast
enough while scrolling, the documented fallback is a server-side, artwork-scoped
short-lived query-string token (so `Poster` could fetch directly) — but that's a real API
addition and only gets proposed with on-device evidence behind it, not preemptively.

### Discovery: no mDNS API

Roku has no equivalent of `NsdManager`/Bonjour browsing built in. `_swarm-peer._udp.local.`
discovery has to be hand-rolled: join `224.0.0.251:5353` on a `roDatagramSocket`, send/parse
raw DNS-SD PTR/SRV/TXT records. Validate TXT fields exactly as strictly as
`LanDiscoveryManager.parse` does on Fire TV (fingerprint must be exactly 64 lowercase hex
chars, ports must parse into `1..65535`) — a malformed/spoofed mDNS record on the LAN is not
a hypothetical concern, it's the same threat model the Fire TV client already defends
against. A manual "enter server address" fallback is required regardless (and doubles as
the entry point for a relay-reached remote server, once that ships).

### Server-side status (as of this writing)

`apps/server/src/http_media.rs` now runs **two** listeners: the original
plain-HTTP one (`http_media_bind`, default `:8546`) and a second **TLS**
listener (`http_media_tls_bind`, default `:8547`) terminated with a leaf
issued fresh at every server start from a dedicated, persisted HTTP CA
(`swarm_p2p::http_tls` — deliberately separate from the QUIC peer identity
cert; see that module's doc comment). `/pair/poll`'s `Approved` response
carries the CA's PEM in an additive `http_ca_pem` field, so a device that
just paired learns its trust anchor at the same moment it learns its bearer
token. `lan.rs`'s mDNS TXT record advertises the TLS port as
`http_media_tls_port` (only present once the listener actually started).
Proven end-to-end in `apps/server/tests/http_media.rs`'s
`pair_over_http_learns_ca_then_media_is_reachable_over_pinned_tls` test: a
`reqwest` client trusting only the returned CA PEM successfully negotiates
and range-fetches real media over the TLS listener, while a client that
doesn't trust it fails at the handshake itself.

**What this does and doesn't solve**: this gets a Roku client TLS-secured
reach to a server it can already route to (same LAN, or a manually
port-forwarded remote address) — a real improvement over plaintext HTTP.
It does **not** yet solve *discovery/routing* to a server the client has no
network path to at all (the genuinely off-LAN, no-port-forward case), which
was scoped in planning as a TLS-passthrough relay through the hosted STUN
service. That relay (a pooled-port TCP passthrough on the STUN server, an
opt-in outbound tunnel from the media server, `relay_addr` published via
device metadata) is **not implemented yet** — tracked as a real gap, not
silently dropped. Until it lands, Roku should be built and shipped as
**LAN-first** (mDNS discovery, or manual host entry for a reachable
address/port-forward), with remote-relay reach as a clearly separate,
later increment. Update this note and `feature-inventory.md`'s
corresponding row the moment the relay actually ships.

### Identity & pairing: no persistent client certificate

Fire TV generates a long-lived self-signed identity cert at first launch and submits its
fingerprint at registration. Roku has no equivalent trust primitive to generate/store
client-side. The HTTP pairing flow (`/pair/begin`/`/pair/poll`) was designed for exactly
this — it mints a **server-issued bearer token** instead of relying on a client-presented
certificate. Roku's "identity" is just that opaque token, persisted in the registry; there
is no client keypair to manage at all. Do not try to port the Fire TV identity-cert
machinery — it solves a problem (QUIC mTLS peer identity) Roku's transport doesn't have.

### Video node capability constraints (validate on real hardware before building on top of them)

- The server's HLS output is **fMP4/CMAF, not MPEG-TS**, with `hls_playlist_type: EVENT`
  (a playlist that grows during a live transcode, then gets `#EXT-X-ENDLIST`). Confirm the
  target minimum Roku OS version's `Video` node handles both of these correctly — this is
  flagged as the single highest-risk technical assumption in the initial build.
- `HttpHeaders`/`HttpAgent` on a `ContentNode` must be confirmed to propagate to **HLS
  segment fetches**, not just the initial master-playlist request — a bearer token that
  only reaches the manifest but not the `.m4s` segments would silently break every HLS
  playback path while direct-play (single-request) kept working, which would be an easy
  false "it works" signal during early testing.
- `HTTPCertificatesFile` has no accompanying host-verification opt-out — this is exactly
  why the relay design issues a CA-signed leaf with real SANs rather than trying to pin a
  bare self-signed leaf (see the server-side relay design). Validate this against a
  throwaway CA/leaf and a plain `roUrlTransfer` GET *before* any relay-dependent client code
  is written on top of the assumption that it works.

## Architecture decisions specific to this port

- **BrighterScript, not raw BrightScript** — compile-time checking (types, unreachable
  code, unresolved symbols) comparable to what Kotlin/KSP gives the Fire TV build, given
  there's no equivalent of Android Studio's live inspection for `.brs`.
- **Task nodes are the coroutine equivalent.** Every network call, every registry write of
  meaningful size, and mDNS socket I/O runs in a Task, never on the render thread.
- **One `AppState` node mirrors the Fire TV `UiState` sealed hierarchy** field-for-field in
  spirit: current screen + its data, `previous` reference for back-navigation, and the same
  set of global flows (notifications, minimized player, kid-mode settings, etc.) as
  observable fields instead of `StateFlow`s. Screens read `AppState` fields and call
  intent-equivalent functions; they never own transport/storage logic directly — same
  discipline as `MainActivity`/`SwarmViewModel`'s split.

## Open items / update this file as they resolve

- [ ] Confirm fMP4/CMAF + EVENT playlist HLS playback on real target hardware.
- [ ] Confirm `HttpHeaders` reaches HLS segment requests, not just the manifest.
- [ ] Confirm CA-signed-leaf + `HTTPCertificatesFile` satisfies hostname verification for
      the relay path.
- [ ] Measure registry write volume against the ~32KB/section cap with a real large
      library's worth of watch-state/likes/watchlist entries; tune the LRU caps from
      measurement, not guesswork.
- [ ] Confirm the `ArtworkTask` pool keeps a 5-wide scrolling grid populated acceptably;
      revisit the query-token fallback idea only if it doesn't.
