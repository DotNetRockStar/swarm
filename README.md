# SWARM

A free, secure, strictly peer-to-peer media streaming suite — Plex-like, but your media never touches a cloud. People who own their music and video stream it from their own machines to their own devices, end-to-end encrypted. The hosted STUN server coordinates devices and relays hole-punch signaling only; it cannot see or decrypt media.

Three apps:

| App | Where | Stack |
|---|---|---|
| **STUN server** (`apps/stun-server`) | hosted (Oracle Cloud free tier, Docker + Caddy on :443) | Rust, Axum, utoipa/Swagger, SQLite |
| **Server app** (`apps/server`) | your macOS/Linux/Windows machine with the media | Tauri (Rust core + web UI), bundled ffmpeg |
| **TV client** (`clients/tv-android`) | Fire TV (first), Amazon Appstore | Kotlin, Compose for TV, Media3/ExoPlayer |

## How it works

1. Register on the STUN server web UI, create a **swarm** (a private device group), and generate an 8-digit join code.
2. Enter the code on a device (server app or TV client). The device registers — submitting its metadata and its self-signed certificate fingerprint — and receives an access token (stored encrypted on-device).
3. Devices in the same swarm find each other via STUN presence, exchange hole-punch candidates over WSS signaling, punch, and connect **directly** over QUIC with mutually pinned certificates. No relay exists; media flows only device-to-device.
4. Clients merge the catalogs of every server in their swarms (keyed on content fingerprints, so the same file on two servers is one entry with two sources) and pick the best source at play time. Direct play when the client can decode the file; otherwise the server transcodes to HLS with an adaptive bitrate ladder.

Full protocol: [docs/PROTOCOL.md](docs/PROTOCOL.md). Design lineage: patterns ported from [Batocera Fleet Federation / Batocera.Drone](../batocera-fleet-federation/) (pairing/pinning, transport selection, library delta-sync, transcode sessions); originals of the recovered protocol references are in [docs/reference/](docs/reference/).

## Repo layout

```
crates/swarm-core/         shared contracts (REST/WSS/peer), fingerprint, entry keys — no I/O
crates/swarm-p2p/          QUIC + pinning, identity certs, reflector client, hole punch, UPnP, loopback proxy
crates/swarm-media/        library scan/store/delta-sync, tags, ffprobe, scrapers, HLS transcode
crates/swarm-stun-client/  device-side STUN registration + WSS session
apps/stun-server/          the hosted rendezvous service
apps/server/               Tauri desktop media server
clients/tv-android/        Fire TV client — Gradle multi-module (:core, :app); see its own README
openapi/                   generated OpenAPI + generated Kotlin client (CI gate)
deploy/                    docker-compose + Caddy for the STUN server
tests/integration/         docker-composed multi-node + simulated-NAT harness
docs/                      PROTOCOL.md, recovered reference implementations
```

## Development

```bash
cargo test --workspace                              # everything except the Tauri GUI
cargo test -p swarm-server --features gui            # include the GUI binary's own tests
cargo run -p stun-server                              # run the STUN server locally
SWARM_MEDIA_ROOT=/path/to/media cargo run -p swarm-server --bin swarm-serverd   # headless media server
SWARM_MEDIA_ROOT=/path/to/media cargo run -p swarm-server --features gui --bin swarm-server-app  # desktop app
```

The fingerprint tests pin byte-for-byte compatibility with the original Python `sample-fp-v1` implementation — do not change `fingerprint.rs` without regenerating vectors against `batocera.drone/app/common/fingerprint.py`.

Server app env vars (headless daemon; the GUI persists the same settings to `<app data dir>/settings.json` instead): `SWARM_MEDIA_ROOT` (required), `SWARM_DATA_DIR`, `SWARM_PEER_BIND`, `SWARM_ALLOW_FPS` (comma-separated fingerprints, for running without a STUN server), `SWARM_STUN_URL`/`SWARM_STUN_CODE`/`SWARM_DEVICE_NAME` (one-shot swarm join at startup), `SWARM_TOKEN_STORE_FILE_ONLY` (skip the OS keyring on headless boxes with no Secret Service).

TV client (`clients/tv-android`, Gradle — see its own README for the full build/test story and what's deliberately not built yet):

```bash
export JAVA_HOME=/opt/homebrew/opt/openjdk@17
cd clients/tv-android
./gradlew :core:test          # wire contracts + STUN client + catalog merger — JDK only
./gradlew :app:assembleDebug  # full APK; needs local.properties -> an Android SDK
```

## Roadmap

Phases 0–6 with exit criteria are tracked in the project plan: contracts → STUN MVP → server library + LAN direct play → TV client MVP → cross-network hole punch → transcode/ABR → polish + Appstore submission. Phase 3 (TV client) has registration, encrypted token storage, and the swarm dashboard working end-to-end against a real STUN server; the peer QUIC transport (and everything downstream of it — merged catalog, playback) is next, gated on a kwik throughput spike per the risk register.
