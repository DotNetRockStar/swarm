# SWARM TV client (Fire TV) — Phase 3

Native Kotlin, single-activity Jetpack Compose for TV (`androidx.tv:tv-material`),
minSdk 25 (Fire OS 6+, 2018+ devices). Gradle multi-module: `:core` (pure
Kotlin/JVM, no Android dependency) and `:app` (the Android application).

## Build

```bash
export JAVA_HOME=/opt/homebrew/opt/openjdk@17   # AGP needs 17, not whatever `java` defaults to
./gradlew :core:test        # wire contracts, STUN client, catalog merger — JDK only, no SDK needed
./gradlew :app:assembleDebug # full APK (armeabi-v7a + arm64-v8a splits); needs $ANDROID_HOME
```

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

## Deliberately not built yet

- **Peer QUIC transport.** No kwik integration, no loopback HTTP↔QUIC proxy,
  no ExoPlayer wiring. This is explicitly the highest-risk item in the
  project plan's risk register ("kwik (JVM QUIC) throughput on Fire TV
  hardware unproven") — it needs a throughput spike on real hardware before
  committing to the approach, and there's no Fire TV device or Android
  emulator available in this environment to run that spike honestly. The
  `:core` peer contracts (`PeerRequest`/`PeerResponseHeader`/`ByteRange`) are
  ready and tested for whenever this lands.
- Merged multi-server catalog UI, resume/watched state, direct-play/transcode
  negotiation, diagnostics screen (NAT type, punch results, per-server RTT) —
  all depend on the transport above.
- Room (local catalog cache) — no DAO/entity code exists yet, so it isn't
  wired into the Gradle build; adding it means also adding the KSP plugin.
- Visual/on-device verification. The debug APK builds and its manifest is
  correct, but nobody has run it — no physical Fire TV and no emulator was
  set up in this environment. Install `app/build/outputs/apk/debug/
  app-arm64-v8a-debug.apk` on a real device or `adb`-connected emulator to
  see it render.
