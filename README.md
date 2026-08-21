# SWARM

A free, secure, strictly peer-to-peer media streaming suite — Plex-like, but your media never touches a cloud. People who own their music and video stream it from their own machines to their own devices, end-to-end encrypted. The hosted STUN server coordinates devices and relays hole-punch signaling only; it cannot see or decrypt media.

## TL;DR — test the Fire TV client against a local STUN + media server

Everything runs on this one machine; the Fire TV just needs to be on the same Wi-Fi/LAN. No mocks — these are the same real binaries the automated tests spawn as subprocesses, just left running.

**1. Start everything:**
```bash
./run_now.sh
```
Builds and runs three real processes together: the STUN server, a headless media server, and the desktop GUI (a window should open). Prints two STUN URLs at the end — always use the **LAN** one, e.g. `http://192.168.0.242:8080`, never `127.0.0.1` (that only works for a browser on this same machine; a real device on your LAN can't reach loopback). `Ctrl+C` stops all three together. If the LAN IP looks wrong (e.g. a `10.x`/`100.x` VPN address instead of your real `192.168.x.x` Wi-Fi one), see [Manual end-to-end testing](#manual-end-to-end-testing-stun--server--real-fire-tv) below.

**2. Get a passcode** (the STUN server's own web UI, no separate tooling):
- Open the **LAN URL** from step 1 in a browser (e.g. `http://192.168.0.242:8080`).
- Click the **Create account** tab, enter any email + a password (**10+ characters**, that's the only rule), and click **Create account** — it signs you in automatically, no email verification needed for local testing.
- Under **Your swarms**, type a name into the **New swarm name** box (e.g. "Home") and click **Create**. A swarm is a private device group; only devices that join the same one can find each other.
- Click the new swarm to expand it, then click **Generate join code**. A popup shows the code (8 digits, shown as two groups of 4) and its expiry — **single-use**, good for **15 minutes**. Every device needs its own fresh code, so you'll come back here once per device (desktop GUI, then Fire TV).

**3. Register the desktop GUI into that swarm:**
In the GUI window `run_now.sh` already opened, click **Choose media folder…** first (required — pick real media if you want something to actually play, or just point it at the sample `.run/media/` folder for a quick connectivity check). Next it shows **Join a swarm (optional)** — fill in **STUN server URL** (the LAN URL from step 1), **Join code** (from step 2), and **Device name** (defaults to "SWARM Server"), then click **Join swarm**. Or click **Skip for now** and do it later from the GUI's **Swarm** tab (same fields, under "Join another swarm").

**4. Install and register the TV client:**
```bash
./deploy_tv.sh 192.168.0.148    # your Fire TV's IP — Settings -> My Fire TV -> About -> Network
```
First time only: enable Developer Options on the Fire TV (**Settings → My Fire TV → About**, click the device name row ~7 times), then turn on **ADB debugging** under the new Developer Options entry, and accept the "Allow USB debugging?" prompt on the TV.

Once installed and launched, the app opens straight to its **STUN server URL** / **Device name** / passcode screen. Enter the same **LAN URL** from step 1, a **fresh** join code (generate a new one on the STUN web UI — the one you used for the GUI is already spent, it's single-use), and click **Join swarm**.

**5. Test it:** on the Fire TV, browse the merged catalog and play something. Both the GUI and the Fire TV should now show up as devices in that swarm on the STUN web UI.

Already have something running from a previous session? `./run_now.sh` now self-heals — it kills anything still holding its ports before starting, so you never need to manually hunt down stale processes first.

Full details, env vars, and troubleshooting: [Manual end-to-end testing](#manual-end-to-end-testing-stun--server--real-fire-tv) below.

Three apps:

| App | Where | Stack |
|---|---|---|
| **STUN server** (`apps/stun-server`) | hosted (Oracle Cloud free tier, Docker + Caddy on :443) | Rust, Axum, utoipa/Swagger, SQLite |
| **Server app** (`apps/server`) | your macOS/Linux/Windows machine with the media | Tauri (Rust core + web UI), FFmpeg/ffprobe runtime |
| **TV client** (`clients/tv-android`) | Fire TV (first), Amazon Appstore | Kotlin, Compose for TV, Media3/ExoPlayer |

## How it works

1. Register on the STUN server web UI, create a **swarm** (a private device group), and generate an 8-digit join code.
2. Enter the code on a device (server app or TV client). The device registers — submitting its metadata and its self-signed certificate fingerprint — and receives an access token (stored encrypted on-device).
3. Devices in the same swarm find each other via STUN presence, exchange hole-punch candidates over WSS signaling, punch, and connect **directly** over QUIC with mutually pinned certificates. No relay exists; media flows only device-to-device.

On the same LAN, STUN is optional: the media server advertises itself over mDNS, the TV client lists it automatically, and a short-lived six-digit code establishes certificate trust the first time. Later LAN connections reuse that persisted trust and go directly over QUIC.
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
deploy/stun-server/        Docker + Compose + Caddy for the hosted STUN server
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

Server app env vars (headless daemon; the GUI persists media/scraper settings to `<app data dir>/settings.json`): `SWARM_MEDIA_ROOT` (required), `SWARM_DATA_DIR`, `SWARM_PEER_BIND`, `SWARM_ALLOW_FPS` (comma-separated fingerprints, for running without a STUN server), `SWARM_STUN_URL`/`SWARM_STUN_CODE`/`SWARM_DEVICE_NAME` (one-shot swarm join at startup), `SWARM_TOKEN_STORE_FILE_ONLY` (skip the OS keyring on headless boxes with no Secret Service).

Streaming bandwidth is a server-wide reservation pool. `SWARM_MAX_UPLOAD_MBPS`
(default `10`) is reduced by `SWARM_UPLOAD_RESERVE_PERCENT` (default `90`,
capped at `90`), and the peak rates reserved by every active playback must
fit in the remainder. `SWARM_MAX_STREAMS` defaults to `2`; `SWARM_FFMPEG_PATH`
selects the FFmpeg binary; `SWARM_TRANSCODING_DISABLED=1` disables HLS while
retaining compatible direct play. Example: at the defaults, a 10 Mbps uplink
leaves only a 1 Mbps pool — under the ~1.1 Mbps even the lowest (360p) HLS
rung needs, so raise `SWARM_MAX_UPLOAD_MBPS` to match your real uplink (or
lower `SWARM_UPLOAD_RESERVE_PERCENT`) if transcoded playback should fit.
Direct play (no transcode) has no such floor — it only needs the source
file's own bitrate to fit the pool.
FFmpeg and ffprobe must be installed on the media server; set
`SWARM_FFMPEG_PATH` when `ffmpeg` is not on `PATH`.

TV client (`clients/tv-android`, Gradle — see its own README for the full build/test story and what's deliberately not built yet):

```bash
export JAVA_HOME=/opt/homebrew/opt/openjdk@17
cd clients/tv-android
./gradlew :core:test          # wire contracts + STUN client + catalog merger — JDK only
./gradlew :app:assembleDebug  # full APK; needs local.properties -> an Android SDK
```

## Manual end-to-end testing (STUN + server + real Fire TV)

Two repo-root scripts drive the full stack for hands-on testing — no
mocks, the same real binaries every automated test spawns as subprocesses
(see `.claude/skills/swarm-interop-test/`), just left running so you can
point a browser or a real device at them.

**1. Start the STUN server + media server(s):**
```bash
./run_now.sh
```
Builds and runs three real processes together: `swarm-stun-server`, a
headless `swarm-serverd`, and the Tauri desktop GUI (`swarm-server-app`) —
all bound to `0.0.0.0` (not just loopback) so a real device on your LAN
can reach them. Prints two STUN URLs — always use the **LAN** one (e.g.
`http://192.168.x.x:8080`), not `127.0.0.1`, for anything other than a
browser on this same machine. If the LAN IP printed looks wrong, this
machine likely has a VPN active; see `.claude/skills/swarm-local-testing/`
for why and how the script already works around it.

Open the LAN URL in a browser, register an account, create a swarm, and
mint an 8-digit join code. In the GUI window the script already opened,
first-run onboarding asks for a media folder first (required — pick real
media, or point it at the sample `.run/media/` folder), then offers an
optional "join a swarm" screen where you paste the LAN URL + that code —
or skip and join later from the Swarm tab. Alternatively, drop real files
directly into `.run/media/`, the headless daemon's own scanned root, and
register *it* into a swarm instead by stopping the script and re-running
with `SWARM_STUN_URL`/`SWARM_STUN_CODE` set (printed in the script's own
output; auto-registers the headless daemon only, never the GUI). Either
server works fine for TV client testing — the GUI is just easier to watch
scan/scrape progress on, the headless one is what every automated test
already exercises.

**2. Install the TV client on a real Fire TV:**
```bash
./deploy_tv.sh 192.168.0.148   # your Fire TV's IP — Settings -> My Fire TV -> About -> Network
```
First time: enable Developer Options on the TV (**Settings → My Fire TV →
About**, click the device name row ~7 times), then turn on **ADB
debugging** under the new **Developer Options** entry, and accept the
one-time "Allow USB debugging?" prompt on the TV screen when it appears.

`deploy_tv.sh` rebuilds the debug APK, installs it via adb, force-stops
any previous run, launches it, and polls for up to 16s to verify it
actually stayed up (checks for `FATAL EXCEPTION` in logcat and confirms
the process is still alive) rather than just trusting a successful
install — this is what caught a real launch crash on first real-hardware
use (see `.claude/skills/swarm-real-device-debugging/` for the full
story). Add `-f` to tail logcat afterward. On the TV, enter the **LAN**
STUN URL from step 1 plus the join code on the passcode screen.

Building against a real device specifically needs the **debug** build —
the release manifest intentionally disables cleartext HTTP/WS traffic for
Appstore compliance, and `run_now.sh` serves plain (non-TLS) endpoints.

## Roadmap

Phases 0–6 with exit criteria are tracked in the project plan: contracts → STUN MVP → server library + LAN direct play → TV client MVP → cross-network hole punch → transcode/ABR → polish + Appstore submission. Direct/punched QUIC playback and upload-budgeted HLS negotiation are implemented; real-device throughput/decoder validation and product-level server bandwidth controls remain before Appstore submission.
