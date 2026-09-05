# Feature-parity inventory

The living tracking artifact for [[swarm-cross-client-parity]]. One row per user-visible
SWARM behavior, status per client. Update this file **in the same change** that alters the
behavior on any client — see [[swarm-cross-client-parity]]'s checklist for the procedure.

Status values: **Complete** (matches Fire TV's behavior exactly) · **Partial** (present but
missing a documented piece) · **Not implemented** · **Deviation** (intentionally different,
reason recorded in the Notes column or the deviation table at the bottom).

Fire TV is the reference column — its rows are "Complete" by definition except where noted
as a deviation candidate itself (rare: only if a Roku/future-platform limitation forces a
Fire TV change too, which should be rare and deliberate).

## Core connectivity

| Feature | Fire TV | Roku | Notes |
|---|---|---|---|
| STUN registration / activation code flow | Complete | Not implemented | Roku: same REST calls, no client cert to generate |
| Join additional swarm by code | Complete | Not implemented | |
| Leave swarm / switch active swarm | Complete | Not implemented | |
| LAN discovery (mDNS) | Complete (`NsdManager`) | Not implemented | Roku: hand-rolled DNS-SD over `roDatagramSocket`, see platform-notes/roku.md |
| LAN pairing | Complete (raw NDJSON, `lan.rs`) | Not implemented | **Deviation**: Roku uses `/pair/begin`+`/pair/poll` (HTTP), a different, purpose-built protocol — not a partial port of the NDJSON one |
| Remote (off-LAN) server reach | Complete (QUIC hole-punch) | Not implemented | **Gap, tracked**: server now issues TLS (CA + leaf, `http_media_tls_bind`/`:8547`, see platform-notes/roku.md) so a *reachable* address can be secured, but the relay that would let Roku *reach* a server with no direct network path (no port-forward) is not built yet. Not a deviation until the relay actually ships and a deliberate final shape is chosen. |
| Disconnect / reconnect / forget a server | Complete | Not implemented | |
| Dashboard presence refresh (10s poll) | Complete | Not implemented | |
| Device/app-build testing mode (debug builds) | Complete | Not implemented | Lower priority — dev/QA convenience, not user-facing |

## Catalog & browsing

| Feature | Fire TV | Roku | Notes |
|---|---|---|---|
| Merged multi-server catalog | Complete | Not implemented | |
| Movies shelf + genre sub-shelves | Complete | Not implemented | |
| Shows shelf (grouped) + genre sub-shelves | Complete | Not implemented | |
| Music/artist shelf (grouped) + genre sub-shelves | Complete | Not implemented | |
| Continue Watching row (cap 6) | Complete | Not implemented | |
| Watchlist row | Complete | Not implemented | |
| Persistent filter rail (kind/liked/genre/rating) | Complete | Not implemented | |
| Search (submit-only, not live-filter) | Complete | Not implemented | |
| "Browse All" grids (alphabetical) | Complete | Not implemented | |
| Genre-filtered full grid | Complete | Not implemented | |
| Live catalog change-feed (long-poll delta) | Complete | Not implemented | |
| Catalog cache (offline/warm-start paint) | Complete (files) | Not implemented | Roku: `cachefs:/` |
| Hover/browse preview playback | Complete | Not implemented | |
| Movie detail screen | Complete | Not implemented | |
| Show → season → episode navigation | Complete | Not implemented | |
| Artist → album → track navigation | Complete | Not implemented | |

## Playback

| Feature | Fire TV | Roku | Notes |
|---|---|---|---|
| Direct-play (Range-served bytes) | Complete | Not implemented | |
| HLS adaptive/remux playback | Complete | Not implemented | fMP4/CMAF — validate on real Roku hardware first |
| Resume from saved position | Complete | Not implemented | |
| Session negotiation discipline (`/stop` on every teardown) | Complete | Not implemented | Load-bearing — see state-model.md |
| Pause overlay (metadata, cast, recommendations) | Complete | Not implemented | |
| Skip intro / recap / credits markers | Complete | Not implemented | |
| "Up next" continue overlay + countdown autoplay | Complete | Not implemented | |
| Next-episode preload | Complete | Not implemented | |
| Buffering-triggered quality recovery | Complete | Not implemented | |
| Audio/subtitle track selection | Complete | Not implemented | |
| Session-expiry recovery (renegotiate on stale 404) | Complete | Not implemented | |
| Music playback screen (lyrics, shuffle, repeat) | Complete | Not implemented | |
| Music minimize to mini-player (Back semantics) | Complete | Not implemented | |
| Music track preload / gapless auto-advance | Complete | Not implemented | |

## Device preferences & state

| Feature | Fire TV | Roku | Notes |
|---|---|---|---|
| Settings: server URL, device name | Complete | Not implemented | |
| Kid Mode (PIN, rules, single-chokepoint filter) | Complete | Not implemented | |
| Watch state persistence (95% threshold) | Complete | Not implemented | **Deviation planned**: LRU-capped on Roku (registry size), unbounded on Fire TV |
| Watchlist persistence | Complete | Not implemented | **Deviation planned**: LRU-capped on Roku |
| Likes persistence | Complete | Not implemented | **Deviation planned**: LRU-capped on Roku |
| Resolved-problem notifications inbox | Complete | Not implemented | **Deviation planned**: LRU-capped on Roku |
| Client-error reporting (auto + user-initiated) | Complete | Not implemented | |
| Client-error local retry queue (cap ~20) | Complete | Not implemented | |

## Images

| Feature | Fire TV | Roku | Notes |
|---|---|---|---|
| Authenticated artwork fetch | Complete | Not implemented | **Deviation**: Roku fetches via Task→cachefs then hands `Poster` a local path (no HTTP-header support on `Poster`) |
| Artwork TTL cache (30 days, refresh-on-hit-only) | Complete | Not implemented | |
| Artwork retry + fallback (artist photo → album cover) | Complete | Not implemented | |
| Shelf-scroll artwork prefetch | Complete | Not implemented | |
| Branded placeholder art (always drawn under real art) | Complete | Not implemented | |

## UX chrome

| Feature | Fire TV | Roku | Notes |
|---|---|---|---|
| Shared palette / button interaction language | Complete | Not implemented | Must be hex-for-hex — see ux-rules.md |
| Toast/notification host (3 severities, cap 4) | Complete | Not implemented | |
| Loading indicator + themed message rotation | Complete | Not implemented | |
| Exit-confirm modal | Complete | Not implemented | |
| Keep-awake during active playback only | Complete | Not implemented | |
| Two-tier filter-rail Back | Complete | Not implemented | |
| Two-tier player Back (video pause-then-exit; music minimize) | Complete | Not implemented | |

## Server/protocol surface consumed by the client

| Feature | Fire TV transport | Roku transport | Notes |
|---|---|---|---|
| Catalog/artwork/playback API | QUIC (`PeerRequest`) via loopback proxy | Plain HTTPS to `http_media.rs` | Same payload shapes both ways — see http-client-contract.md |
| Pairing | Raw NDJSON (`lan.rs`) or STUN activation | `/pair/begin` + `/pair/poll` | Different protocols, same product outcome |
| Capability profile | `MediaCodecList`/`Display` probe | `roDeviceInfo` probe | Same merge algorithm, different probe source |

---

## Recorded intentional platform-specific deviations

A deviation is only "intentional" once it has a row here with a reason. An
undocumented difference discovered later during a parity review must either become a bug
(fix it) or get promoted to a documented deviation (record why) — it can never stay silent.

| Behavior | Fire TV | Roku | Why |
|---|---|---|---|
| Remote/off-LAN server transport | QUIC hole-punch | Planned: TLS-passthrough relay (not yet built — see the gap row above and platform-notes/roku.md) | No QUIC in BrightScript; a relay is the only way to preserve the "server never sees plaintext" promise for a device with no other path to a server (see plan/issue #54) |
| LAN pairing protocol | Raw NDJSON over TCP (`lan.rs`) | HTTP JSON (`/pair/begin`/`/pair/poll`) | Roku has no client cert to present; the HTTP flow mints a bearer token instead |
| Server discovery mechanism | `NsdManager` (OS mDNS) | Hand-rolled DNS-SD over `roDatagramSocket` | No mDNS API on Roku |
| Durable per-item state (watch/likes/watchlist/notifications) | Unbounded (Room/SharedPreferences) | LRU-capped (registry ~32KB/section) | Roku registry has a hard size ceiling; Fire TV's storage does not |
| Media→player bridge | `PeerLoopbackProxy` (local HTTP↔QUIC translation) | None — `Video` node consumes server HTTP(S) URLs directly | Roku's player already speaks HTTP; the bridge only exists to work around QUIC |
| Client identity | Long-lived self-signed keypair, cert fingerprint pinned at registration | Server-issued opaque bearer token, no client keypair | Roku's transport has no mTLS peer-identity concept to anchor a cert to |
| Transport controls UI | Media3 native `PlayerView` controller | Custom SceneGraph overlay | No built-in equivalent on Roku; also avoids porting Fire TV's own #154-class "native controller keeps focus after hiding" bug class by construction |
| Artwork delivery to the image widget | Coil loads authenticated URL directly | Task fetches → `cachefs:/` → `Poster` reads local file | `Poster` has no HTTP header support |

## How to update this file

1. Any change to a feature/behavior on any client: update that client's status cell in the
   same change.
2. Marking something a **Deviation** requires adding (or already having) a row in the
   deviation table above with a real reason — "ran out of time" is not a deviation reason,
   it's a **Not implemented** status plus a follow-up issue.
3. See [[swarm-cross-client-parity]] for the full procedure and the rule this file exists
   to serve.
