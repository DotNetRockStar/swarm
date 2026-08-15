# SWARM TV client (Fire TV) — Phase 3

Native Kotlin, single-activity Jetpack Compose for TV (`androidx.tv:tv-material`),
minSdk 25 (Fire OS 6+, 2018+ devices). Gradle multi-module: `:core` (pure
Kotlin/JVM, no Android dependency) and `:app` (the Android application).

## Build

```bash
export JAVA_HOME=/opt/homebrew/opt/openjdk@17   # AGP needs 17, not whatever `java` defaults to
./gradlew :core:test         # wire contracts, STUN client, catalog merger, PeerQuicClient — JDK only, no SDK needed
./gradlew :core:interopTest  # kwik <-> real Rust quinn server QUIC spike; needs a release swarm-serverd build (see below)
./gradlew :app:assembleDebug # full APK (armeabi-v7a + arm64-v8a splits); needs $ANDROID_HOME
```

`:core:interopTest` needs `cargo build --release -p swarm-server --bin swarm-serverd` run first from the
workspace root — it skips gracefully (not fails) if that binary isn't present, so it's a separate
opt-in task rather than part of the default `:core:test`.

`local.properties` (gitignored) must point `sdk.dir` at an installed Android SDK
with `platforms;android-35` + `build-tools;35.0.0`.

`:app:lintDebug` currently crashes on this AGP 8.7.3 / Kotlin 2.0.21 combination
— an `IncompatibleClassChangeError` inside stock lint detectors
(`NonNullableMutableLiveDataDetector`, `RememberInCompositionDetector`) that's a
toolchain version-skew bug, not a real finding (`compileDebugKotlin` and
`assembleDebug` both succeed cleanly). Revisit on the next AGP/Kotlin bump.

## What's built and verified

- **`:core`** — wire contracts hand-mirrored from `swarm-core` (Rust): REST
  (`rest/Contracts.kt`) and peer/catalog (`peer/Contracts.kt`) types, using
  kotlinx.serialization with a `JsonNamingStrategy.SnakeCase` config that
  reproduces serde's `rename_all`. `ByteRange` needed a hand-written
  `KSerializer` since serde's default externally-tagged enum representation
  (`{"from_to": {...}}`) doesn't match kotlinx.serialization's default
  internally-tagged one. **Every fixture in the test suite was captured by
  actually running `serde_json::to_string` against equivalent Rust values**
  (see the doc comments on `ContractsTest.kt` in both `rest/` and `peer/`) —
  not hand-guessed JSON — so the 24 passing `:core` tests are a real
  cross-language compatibility proof, not just internal self-consistency.
  Also: `StunApiClient` (OkHttp, mirrors `swarm-stun-client::client::StunClient`
  method-for-method, tested against MockWebServer) and `CatalogMerger` (the
  same-fingerprint-on-two-servers-is-one-entry merge rule from
  `docs/PROTOCOL.md`, fully unit tested).
- **`:app`** — compiles into a real, installable debug APK. Manifest verified
  via `aapt dump badging`: `LEANBACK_LAUNCHER` present,
  `android.hardware.touchscreen` explicitly not required,
  `android.software.leanback` required, no Google-Play-Services dependency.
  Screens: passcode entry (STUN URL + device name as free text — the one
  place this app uses phone-style Material3 fields, since TV Material3 has
  no text-entry component — plus the 8-digit join code via a D-pad-navigable
  number grid, not the system keyboard) and a swarm dashboard (server roster
  with online/offline status, resync button). `AndroidTokenStore` wraps
  `androidx.security.crypto.EncryptedSharedPreferences` (Keystore-backed
  AES256-GCM) for the access token; `AndroidDeviceIdentity` generates a real
  `AndroidKeyStore` EC keypair with an auto-issued self-signed certificate
  (non-exportable private key — a `KeyManagerFactory` over this same alias is
  the standard way to hand it to a TLS/QUIC stack later) and exposes its
  SHA-256 fingerprint, submitted at registration exactly like the Rust
  server's `swarm-p2p::identity`.
- Registration end-to-end is real: passcode submission calls the actual STUN
  REST API, saves the returned token encrypted, and loads the swarm's device
  roster.
- **`PeerQuicClient`** (`:core/transport`) — a kwik-based QUIC client mirroring
  `swarm_p2p::endpoint::connect`/`send_request` (Rust): mTLS with a client
  certificate, post-handshake fingerprint pinning (kwik's builder has no
  CA-less pinning callback, so this verifies `getServerCertificateChain()`
  itself and closes on mismatch — see the class doc), and the
  one-request-per-QUIC-stream framing. **Proven against the real Rust server**,
  not just compiled: `PeerQuicClientInteropTest` (`./gradlew :core:interopTest`)
  spawns the actual release `swarm-serverd` binary and drives it from Kotlin
  over loopback QUIC — a full session (thumbprint, manifest, a byte-exact
  300 KB direct-play transfer, a seek Range, a suffix Range) passes
  repeatedly with every byte verified, and a client whose certificate isn't
  on the server's allow-list is correctly refused. See that test class's doc
  comment for the one open finding: running the three interop tests back to
  back is occasionally flaky (`IOException: Connection closed` on one of
  several connections made in quick succession) even though every isolated
  run and the full multi-request single-connection session are 100%
  reliable — undiagnosed, flagged for follow-up before leaning on kwik for a
  reconnect-heavy path (e.g. Phase 4's hole-punch retry loop), not blocking
  the "kwik is a viable QUIC stack for this protocol" conclusion.

## Deliberately not built yet

- **The loopback HTTP↔QUIC proxy and ExoPlayer wiring.** `PeerQuicClient`
  moves bytes correctly; what's missing is the layer that makes a media
  player able to use it — a local `127.0.0.1` HTTP server translating
  ExoPlayer's requests (including `Range`) into `PeerQuicClient.request()`
  calls, per `docs/PROTOCOL.md`'s loopback-proxy note. Mechanical work on top
  of a now-proven transport, not a research question anymore.
- **Real hardware throughput.** The project plan's risk register asks for a
  kwik throughput spike *on Fire TV hardware specifically* — no physical
  device or Android emulator is available in this environment, so only the
  loopback-over-localhost interop (above) could be verified here. Loopback
  proves correctness, not throughput/latency under real wifi and a
  constrained TV CPU.
- Merged multi-server catalog UI, resume/watched state, direct-play/transcode
  negotiation, diagnostics screen (NAT type, punch results, per-server RTT),
  the hole-punch candidate exchange itself (this client dials a known
  host:port directly; it doesn't yet gather/exchange candidates over WSS
  signaling) — all build on the transport above but aren't wired up yet.
- Room (local catalog cache) — no DAO/entity code exists yet, so it isn't
  wired into the Gradle build; adding it means also adding the KSP plugin.
- Visual/on-device verification of the `:app` UI. The debug APK builds and
  its manifest is correct, but nobody has run it — no physical Fire TV and
  no emulator was set up in this environment. Install
  `app/build/outputs/apk/debug/app-arm64-v8a-debug.apk` on a real device or
  `adb`-connected emulator to see it render.
