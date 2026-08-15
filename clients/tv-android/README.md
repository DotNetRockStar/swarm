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
  not hand-guessed JSON — so those `:core` tests are a real cross-language
  compatibility proof, not just internal self-consistency. Also:
  `StunApiClient` (OkHttp, mirrors `swarm-stun-client::client::StunClient`
  method-for-method, tested against MockWebServer) and `CatalogMerger` (the
  same-fingerprint-on-two-servers-is-one-entry merge rule from
  `docs/PROTOCOL.md`, fully unit tested). 38 tests, all passing.
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
  one-request-per-QUIC-stream framing.
- **`PeerLoopbackProxy`** (`:core/proxy`) — bridges standard HTTP (what
  Media3/ExoPlayer, or any off-the-shelf client, speaks) to the peer
  protocol: hands out `http://127.0.0.1:<port>/<serverId>/<peerPath>` URLs,
  translates each request (`Range`, `If-None-Match` included) into a
  `PeerConnection.request()` call, and streams the response back with
  correct status/headers. Programs against a small `PeerConnection`
  interface (not `PeerQuicClient` directly) so its HTTP-translation logic is
  unit tested fast, against a fake, with no live QUIC connection required.
- **Both proven against the real Rust server, not just compiled.**
  `PeerQuicClientInteropTest` (`./gradlew :core:interopTest`) spawns the
  actual release `swarm-serverd` binary and drives it from Kotlin over
  loopback QUIC:
  - A full `PeerQuicClient` session — thumbprint, manifest, a byte-exact
    300 KB direct-play transfer, a seek Range, a suffix Range — passes
    repeatedly with every byte verified, and a client whose certificate
    isn't on the server's allow-list is correctly refused.
  - **The complete story minus ExoPlayer itself**: a plain HTTP client
    (OkHttp, standing in for ExoPlayer's own data source) fetches the
    manifest, the full file, and a mid-file seek — all through
    `PeerLoopbackProxy`, wired to a real `PeerQuicClient`, over real QUIC,
    from the real Rust server — and gets back correct status codes,
    `Content-Range` headers, and byte-exact bodies. If a generic HTTP
    client works here, Media3's `DefaultHttpDataSource` (which speaks
    nothing more exotic than this) will too.
  - See the test class's doc comment for the one open finding: running
    several of these tests back to back is occasionally flaky
    (`IOException: Connection closed` on one connection in the cluster)
    even though every isolated run and the full multi-request sessions are
    100% reliable — undiagnosed, flagged for follow-up before leaning on
    kwik for a reconnect-heavy path (e.g. Phase 4's hole-punch retry loop),
    not blocking the "kwik is a viable QUIC stack for this protocol"
    conclusion.
- **Servers self-report where they are; a client can now dial them from a
  bare STUN roster.** `SwarmDevice.metadata["peer_addr"]` (the Rust side's
  `swarm-p2p::local_addr` self-report, wire-identical to
  `SocketAddr::to_string()`) is parsed by `PeerAddress.parse` (`:core/transport`,
  handles both `host:port` and bracketed `[host]:port` IPv6, unit tested against
  malformed input — never throws, returns null so one bad/not-yet-ready
  roster entry doesn't take down the rest) and fed to `PeerQuicClient.connect`
  via `connectToServer(device, ...)`. `CatalogSession` (`:core/catalog`) is
  the piece that actually uses this: given a swarm roster, it connects to
  every server with a usable `peer_addr`, registers each live connection
  with a `PeerLoopbackProxy`, fetches and merges their catalogs via
  `CatalogMerger`, and reports which devices weren't reachable rather than
  failing the whole refresh. Proven against a real `swarm-serverd`: a
  roster with one real server (dialed purely from its self-reported
  address, no hardcoded host:port anywhere in the test) plus one
  never-registered device merges to the right one-entry catalog and streams
  the file through the proxy correctly.
- **Real finding: kwik cannot yet hold two concurrent connections in one
  process.** The obvious next step from the single-server proof above — a
  `CatalogSession` connected to *two* real servers at once, exactly what a
  multi-server swarm needs for merged browsing — reliably crashes kwik: the
  second connection's receiver thread throws `AssertionError` inside
  `tech.kwik.core.recovery.LossDetector.detectLostPackets` and the
  connection is torn down. 100% reproducible (not the occasional flakiness
  above), on both kwik 0.10.3 and the latest 0.11 (tried upgrading
  specifically to check whether this was already fixed upstream — it
  wasn't, and 0.11 also regressed a previously-reliable single-connection
  test, so stayed on 0.10.3). The test is in
  `PeerQuicClientInteropTest.kt`, marked `@Disabled` with the full
  writeup rather than deleted, so it's easy to re-check after a future kwik
  release. This sharpens the project plan's kwik risk-register entry from
  "throughput on real hardware unproven" to a concrete correctness blocker
  for multi-server swarms specifically; the plan's quiche-JNI fallback is
  the mitigation path if it isn't fixed upstream first.

## Deliberately not built yet

- **Real hardware throughput.** The project plan's risk register asks for a
  kwik throughput spike *on Fire TV hardware specifically* — no physical
  device or Android emulator is available in this environment, so only the
  loopback-over-localhost interop (above) could be verified here. Loopback
  proves correctness, not throughput/latency under real wifi and a
  constrained TV CPU.
- **ExoPlayer wiring itself.** The proxy is proven with a generic HTTP
  client; pointing an actual `MediaItem`/`ExoPlayer` at
  `proxy.urlFor(serverId, peerPath)` and confirming playback is Android-UI
  work that needs a device/emulator to see run.
- **Merged multi-server catalog UI.** `CatalogSession` (see above) is the
  data layer this needs and works for one server; the actual Compose screen
  (rows, grid, focus handling) isn't built, and multi-server merging is
  currently gated on the kwik concurrent-connection bug documented above —
  worth resolving or working around before building UI that assumes it.
- Resume/watched state, direct-play/transcode negotiation, diagnostics
  screen (NAT type, punch results, per-server RTT), the hole-punch candidate
  exchange itself (this client dials a known host:port directly — or now, a
  self-reported `peer_addr` on the same LAN; it doesn't yet gather/exchange
  candidates over WSS signaling for the cross-network case) — all build on
  the transport above but aren't wired up yet.
- Getting `AndroidDeviceIdentity`'s cert/key into `connectToServer`.
  `connectToServer`/`CatalogSession` take a plain `X509Certificate` +
  `PrivateKey`; `AndroidDeviceIdentity` deliberately keeps the private key
  non-exportable inside `AndroidKeyStore` (see its doc comment). Whether
  kwik's `clientCertificateKey(PrivateKey)` works correctly with a
  non-exportable AndroidKeyStore key handle — as opposed to an in-memory one
  like every test here uses — is unverified; needs a device/emulator either
  way, so it's untested regardless.
- Room (local catalog cache) — no DAO/entity code exists yet, so it isn't
  wired into the Gradle build; adding it means also adding the KSP plugin.
- Visual/on-device verification of the `:app` UI. The debug APK builds and
  its manifest is correct, but nobody has run it — no physical Fire TV and
  no emulator was set up in this environment. Install
  `app/build/outputs/apk/debug/app-arm64-v8a-debug.apk` on a real device or
  `adb`-connected emulator to see it render.
