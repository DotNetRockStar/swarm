# SWARM

A free, secure, strictly peer-to-peer media streaming suite — Plex-like, but your media never touches a cloud. People who own their music and video stream it from their own machines to their own devices, end-to-end encrypted. The hosted STUN server coordinates devices and relays hole-punch signaling only; it cannot see or decrypt media.

## TL;DR — test the Fire TV client against a local STUN + media server

Everything runs on this one machine; the Fire TV just needs to be on the same Wi-Fi/LAN. No mocks — these are the same real binaries the automated tests spawn as subprocesses, just left running.

**1. Start everything:**
```bash
./scripts/run_now.sh
```
Builds and runs two real processes together: the hosted-style SWARM service and the desktop media-server app (a window should open). The desktop app is the media server; there is no second headless server. Closing its window hides it to the system tray so streaming continues, and the app asks the operating system to keep the computer awake until **Quit SWARM** is selected in the tray menu. `Ctrl+C` stops both development processes together. Some laptops restrict blocking lid-close sleep (for example, based on power or external-display state), so the operating system's hardware policy still applies in those configurations.

**2. Configure the desktop media server:**
In the window `run_now.sh` opened, choose a media folder. With the development SWARM service URL supplied by the script, the desktop app securely creates and owns its swarm automatically. The service remains infrastructure rather than something an ordinary user has to configure.

**3. Install the TV client:**
```bash
./scripts/deploy_fire_tv.sh    # scans the LAN for Fire TVs and prompts which to deploy to
./scripts/deploy_fire_tv.sh 192.168.0.148    # or target an IP directly — Settings -> My Fire TV -> About -> Network
```
First time only: enable Developer Options on the Fire TV (**Settings → My Fire TV → About**, click the device name row ~7 times), then turn on **ADB debugging** under the new Developer Options entry, and accept the "Allow USB debugging?" prompt on the TV.

On a first-run TV choose **Connect through SWARM**. From an already-connected TV use **Swarm → Add Server → Show Code**. The TV displays a short-lived activation code without asking for a service URL.

**4. Approve the TV:**
Open the desktop app's **Swarm** page, enter the activation code, verify the TV name shown, and approve it. The TV detects approval automatically and opens the swarm dashboard. LAN-discovered servers can still be paired directly without using the internet service.

**5. Test it:** on the Fire TV, browse the merged catalog and play something. Both the GUI and the Fire TV should now show up as devices in that swarm on the STUN web UI.

Already have something running from a previous session? `./scripts/run_now.sh` now self-heals — it kills anything still holding its ports before starting, so you never need to manually hunt down stale processes first.

Full details, env vars, and troubleshooting: [Manual end-to-end testing](#manual-end-to-end-testing-stun--server--real-fire-tv) below.

Three apps:

| App | Where | Stack |
|---|---|---|
| **STUN server** (`apps/stun-server`) | hosted (Oracle Cloud free tier, Docker + Caddy on :443) | Rust, Axum, utoipa/Swagger, SQLite |
| **Server app** (`apps/server`) | your macOS/Linux/Windows machine with the media | Tauri (Rust core + web UI), FFmpeg/ffprobe runtime |
| **TV client** (`clients/tv-android`) | Fire TV (first), Amazon Appstore | Kotlin, Compose for TV, Media3/ExoPlayer |

## TL;DR — run the automated test suites

Five scripts close the loop at different levels — from no-hardware backend
checks up to real Fire TV UI. Full detail (scenario catalogs, evidence
bundles, device targeting) is in `scripts/tests/TV_TESTING.md`; all are
change-controlled — read `.claude/skills/swarm-e2e-suite-lockdown/` before
editing any of their test logic.

```bash
cargo test --workspace                       # 0. plain Rust unit tests — no server, no hardware
./scripts/run_now.sh                         # 1. start the local media/STUN server (separate terminal — this blocks)

./scripts/tests/full_uat_suite.sh --github-issue   # 2. runs everything below in order, one consolidated report/issue
```

`full_uat_suite.sh` is the one-command entry point: it runs
`media_server_uat_tests.sh` → `tv_e2e_suite.sh` → `tv_uat_suite.sh` in
sequence, captures each one's own evidence, and — only when at least one
test actually failed, and only with `--github-issue` — files a single
GitHub issue with every suite's result (passes and failures both), instead
of each suite filing its own. `--skip-backend`/`--skip-e2e`/`--skip-uat`
narrow the run; `--include-resilience` adds the opt-in disruptive suite;
`--device <x>`/`--all` forward to the two hardware suites. Exit code `0`
only if nothing failed. Each wrapped script also still runs fine standalone
(useful for iterating on one layer):

```bash
./scripts/tests/media_server_uat_tests.sh          # media server backend UAT — no hardware, ~1 sec
./scripts/tests/tv_e2e_suite.sh                    # fast smoke test — no UI navigation, full device fan-out
./scripts/tests/tv_uat_suite.sh --github-issue     # full UAT — real Fire TV UI, ~16 scenarios, files its own issue on FAIL
./scripts/tests/tv_uat_resilience_suite.sh         # opt-in: disruptive transport drop/recovery, kept out of the above by design
```

`run_now.sh` runs in the foreground (`Ctrl+C` stops it) — start it once in
its own terminal/session and leave it running; every suite above only
health-checks the server, none of them start or stop it. Only the backend
suite needs no server and no hardware at all, so run it first on any
backend change.

## How it works

1. Register on the STUN server web UI, create a **swarm** (a private device group), and generate an 8-digit join code.
2. Enter the code on a device (server app or TV client). The device registers — submitting its metadata and its self-signed certificate fingerprint — and receives an access token (stored encrypted on-device).
3. Devices in the same swarm find each other via STUN presence, exchange hole-punch candidates over WSS signaling, punch, and connect **directly** over QUIC with mutually pinned certificates. No relay exists; media flows only device-to-device.

On the same LAN, STUN is optional: the media server advertises itself over mDNS, the TV client lists it automatically, and a short-lived six-digit code establishes certificate trust the first time. Later LAN connections reuse that persisted trust and go directly over QUIC.
4. Clients merge the catalogs of every server in their swarms (keyed on content fingerprints, so the same file on two servers is one entry with two sources) and pick the best source at play time. Direct play when the client can decode the file; otherwise the server transcodes to HLS with an adaptive bitrate ladder.

Full protocol: [docs/PROTOCOL.md](docs/PROTOCOL.md). Whole-system, audience-switching walkthrough (devices, media server, STUN server, LAN, security, technology choices, and the test strategy — toggle **User / Engineer**): [docs/guide/index.html](docs/guide/index.html), open it in a browser. Design lineage: patterns ported from [Batocera Fleet Federation / Batocera.Drone](../batocera-fleet-federation/) (pairing/pinning, transport selection, library delta-sync, transcode sessions); originals of the recovered protocol references are in [docs/reference/](docs/reference/).

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
tests/docs/                guardrails for docs/guide — runs in `cargo test --workspace`
docs/                      PROTOCOL.md, docs/guide/ (interactive system guide), recovered reference implementations
```

## Development

```bash
cargo test --workspace                # Rust workspace tests
cargo run -p stun-server              # run the SWARM rendezvous service locally
cargo run -p swarm-server             # run the desktop media server
cd apps/server && npm install && npm run build  # native desktop package
```

The fingerprint tests pin byte-for-byte compatibility with the original Python `sample-fp-v1` implementation — do not change `fingerprint.rs` without regenerating vectors against `batocera.drone/app/common/fingerprint.py`.

The desktop app persists media, scraper, streaming, transcoding, and AI settings in `<app data dir>/settings.json`. Technical overrides are `SWARM_PEER_BIND`, `SWARM_RENDEZVOUS_URL` (the public SWARM service, which can also be compiled into a release), `SWARM_MAX_UPLOAD_MBPS`, `SWARM_UPLOAD_RESERVE_PERCENT`, `SWARM_MAX_STREAMS`, `SWARM_FFMPEG_PATH`, `SWARM_TRANSCODING_DISABLED`, `SWARM_VIDEO_ENCODER`, `SWARM_MAX_TRANSCODE_HEIGHT`, and `SWARM_HLS_SEGMENT_SECONDS`.

On macOS, media roots can connect directly to a NAS from either first-run
onboarding or **Details → Media roots**. Enter the SMB server, share name, and
optional username. Passwords stay in the macOS connection prompt/Keychain and
are never passed to or stored by SWARM. On Linux and Windows, mount the SMB
share with the operating system, then add its local mount path as a media root.

TV builds receive the public service address at build time, keeping it out of the living-room UI: `SWARM_RENDEZVOUS_URL=https://swarm.example.com ./gradlew :app:assembleDebug` (or Gradle property `-PswarmRendezvousUrl=...`). On first connection the TV displays an eight-digit, ten-minute activation code; enter that code on the media server's **Swarm** page and approve the device shown. Existing account-created swarms and join codes remain available as a compatibility fallback.

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

A transcode only runs when the client genuinely can't play the source: when
the client's codec, resolution, bit depth, HDR, and level all fit, playback
is direct, and on LAN a source the client can decode but whose container or
audio track needs changing is remuxed with `-c:v copy` (no re-encode). The TV
client probes its own decoders (`MediaCodecList`) and display (HDR, panel
resolution) at startup and advertises the result, so an HEVC/4K/HDR-capable
Fire TV receives those sources untouched. When a transcode is unavoidable,
**Details → Transcoding** exposes: the video encoder (`SWARM_VIDEO_ENCODER` =
`auto` \| `hardware` \| `software`; `auto` uses the hardware VideoToolbox
encoder on macOS when FFmpeg has it and it has not failed recently), a
resolution ceiling (`SWARM_MAX_TRANSCODE_HEIGHT`, `0` = no cap), and the HLS
segment length (`SWARM_HLS_SEGMENT_SECONDS`, default `4`, minimum `2` —
shorter recovers from a stall faster). Multichannel audio the client lists is
copied or transcoded to AC-3 rather than always downmixed to stereo.

Local English subtitle generation is optional under **Details → Local
subtitles**. Enabling it downloads and verifies the official 466 MiB
`small.en` Whisper model on first use, then processes movies and episodes in
durable ten-minute sections. Completed sections survive a real app restart;
the worker also pauses automatically while a client is streaming. Its
always-visible progress panel is at the top of **Media**, and completed WebVTT
tracks appear in the TV player's normal subtitle controls. Source media never
leaves the media server. Building the desktop app requires CMake because
`whisper.cpp` is linked into the native binary; end users do not install a
separate transcription executable.

TV client (`clients/tv-android`, Gradle — see its own README for the full build/test story and what's deliberately not built yet):

```bash
export JAVA_HOME=/opt/homebrew/opt/openjdk@17
cd clients/tv-android
./gradlew :core:test          # wire contracts + STUN client + catalog merger — JDK only
./gradlew :app:assembleDebug  # full APK; needs local.properties -> an Android SDK
```

## Manual end-to-end testing (STUN + server + real Fire TV)

Two scripts in `scripts/` drive the full stack for hands-on testing — no
mocks, the same real binaries every automated test spawns as subprocesses
(see `.claude/skills/swarm-interop-test/`), just left running so you can
point a browser or a real device at them.

**1. Start the STUN server + media server(s):**
```bash
./scripts/run_now.sh
```
Builds and runs two real processes together: `swarm-stun-server` and the
Tauri desktop media server (`swarm-server-app`). The desktop process owns
both the UI and `ServerCore`; hiding the window does not stop streaming. Both
are bound to `0.0.0.0` (not just loopback) so a real device on your LAN
can reach them. Prints two SWARM service URLs — use the **LAN** one (e.g.
`http://192.168.x.x:8080`), not `127.0.0.1`, for anything other than a
browser on this same machine. If the LAN IP printed looks wrong, this
machine likely has a VPN active; see `.claude/skills/swarm-local-testing/`
for why and how the script already works around it.

In the GUI window the script opened, choose a media folder. The app
automatically creates and manages its own swarm because `run_now.sh` supplied
the rendezvous URL. On the TV choose **Connect through SWARM** on first run, or
**Add Server → Show Code** from an existing dashboard, then enter the displayed
short-lived code on the desktop app's **Swarm** page. The TV returns to its
existing dashboard if that flow is cancelled or the code request fails.

**2. Install the TV client on a real Fire TV:**
```bash
./scripts/deploy_fire_tv.sh                  # scans the LAN for Fire TVs and prompts which to deploy to
./scripts/deploy_fire_tv.sh 192.168.0.148    # or target an IP directly — Settings -> My Fire TV -> About -> Network
```
First time: enable Developer Options on the TV (**Settings → My Fire TV →
About**, click the device name row ~7 times), then turn on **ADB
debugging** under the new **Developer Options** entry, and accept the
one-time "Allow USB debugging?" prompt on the TV screen when it appears.

With no IP argument and no single device already in `adb devices`,
`deploy_fire_tv.sh` scans this Mac's LAN for hosts with adb's port open and
manufacturer `Amazon`, lists each as `name | ip`, and prompts for which one
(or all of them) to deploy to. It then rebuilds the debug APK, installs it
via adb, force-stops any previous run, launches it, and polls for up to 16s
per device to verify it actually stayed up (checks for `FATAL EXCEPTION` in
logcat and confirms the process is still alive) rather than just trusting a
successful install — this is what caught a real launch crash on first
real-hardware use (see `.claude/skills/swarm-real-device-debugging/` for the
full story). Add `-f` to tail logcat afterward (single target only). Approve
each TV's activation code in the desktop app; the service URL is supplied by
the debug build rather than typed with a remote.

Building against a real device specifically needs the **debug** build —
the release manifest intentionally disables cleartext HTTP/WS traffic for
Appstore compliance, and `run_now.sh` serves plain (non-TLS) endpoints.

## Roadmap

Phases 0–6 with exit criteria are tracked in the project plan: contracts → STUN MVP → server library + LAN direct play → TV client MVP → cross-network hole punch → transcode/ABR → polish + Appstore submission. Direct/punched QUIC playback and upload-budgeted HLS negotiation are implemented; real-device throughput/decoder validation and product-level server bandwidth controls remain before Appstore submission.
