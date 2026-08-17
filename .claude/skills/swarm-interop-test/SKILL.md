---
name: swarm-interop-test
description: Use when writing a new test that needs a real swarm-stun-server and/or swarm-serverd binary running (Rust integration tests spawning another Rust binary, or Kotlin tests proving the JVM/kwik side against real Rust processes) - covers the subprocess-spawn, readiness-detection, and cleanup pattern this repo uses everywhere instead of mocks.
---

# Writing a test against real spawned SWARM binaries

This project's standing rule: prove protocol/network behavior against the
**real** binaries, not mocks. Every claim like "kwik interoperates with
quinn" or "a Kotlin client can hole-punch to a real Rust server" is backed
by a test that actually spawns `swarm-stun-server` and/or `swarm-serverd`
as a subprocess. Don't write a fake/mock server for something this repo
already has a real implementation of — spawn the real thing.

## Two flavors

**Rust-to-Rust** (e.g. `apps/server/tests/stun_roster_sync.rs`,
`apps/server/tests/punch_connect.rs`): the STUN server runs **in-process**
in the same test binary via `axum::serve(...)` on a background
`tokio::spawn` — no subprocess needed, since it's the same language and
crate graph. `swarm-serverd`-equivalent logic runs directly via
`ServerCore::start(...)`, also in-process. Add `stun-server = { path =
"../stun-server" }` as a dev-dependency to reach it (a dev-dependency
cycle back to a crate that doesn't depend on you is fine — Cargo allows
it).

**Kotlin-to-Rust** (e.g. `core/src/test/kotlin/.../PeerQuicClientInteropTest.kt`,
`SignalingClientInteropTest.kt`, `PunchConnectInteropTest.kt`): the Rust
side genuinely can't run in-process (different runtime), so these spawn
the real **release** binary as an OS subprocess. This is slower and needs
the binary pre-built, so it's a separate Gradle task
(`:core:interopTest`, not `:core:test`) that skips gracefully rather than
failing when the binary is missing.

## The Kotlin subprocess pattern (copy this shape)

```kotlin
private fun resolveServerBinary(): File? =
    File("../../../target/release/swarm-serverd").canonicalFile.takeIf { it.exists() }
    // or swarm-stun-server — path is core/ -> tv-android/ -> clients/ -> swarm/ -> target/release/...

@BeforeEach
fun setUp() {
    assumeTrue(resolveServerBinary() != null, "release binary not found — run `cargo build --release -p ... --bin ...` first")
}

@AfterEach
fun tearDown() {
    process?.destroyForcibly()?.waitFor(5, TimeUnit.SECONDS)
    // if you used multiple processes/temp dirs in one test, track them in
    // mutable lists (`processes`, `tempDirs`) and iterate here — see
    // PunchConnectInteropTest.kt for the multi-process shape.
}

private fun startServer(...): RunningServer {
    val builder = ProcessBuilder(binary.absolutePath).redirectErrorStream(true)
    builder.environment().apply {
        put("SWARM_...", ...)   // see below for the exact env vars each binary reads
        put("RUST_LOG", "info")
    }
    val proc = builder.start()
    // Block reading stdout line-by-line until a regex match confirms
    // readiness — NEVER a fixed sleep. Every binary logs what you need on
    // startup; strip ANSI codes defensively even though main.rs disables
    // color on non-tty output, in case that ever regresses:
    val line = rawLine.replace(Regex("\\[[0-9;]*m"), "")
    // Then drain the rest of stdout on a daemon thread so the pipe never
    // fills and blocks the subprocess:
    Thread({ generateSequence { reader.readLine() }.forEach { println("[tag] $it") } }, "tag-drain")
        .apply { isDaemon = true }.start()
}
```

### Readiness regexes that already work (don't reinvent)

- `swarm-serverd`: `addr=(\d+\.\d+\.\d+\.\d+:\d+)` (peer QUIC listener) and
  `fingerprint=([0-9a-f]{64})` (device identity) — both logged at startup;
  wait for both if you need the identity too. `"joined swarm via
  SWARM_STUN_CODE"` (a literal substring, no capture group) confirms
  auto-registration completed if you set `SWARM_STUN_URL`/`SWARM_STUN_CODE`.
- `swarm-stun-server`: `bind=(\d+\.\d+\.\d+\.\d+:\d+)` — this is the
  **resolved** listener address (fixed to be this way; it used to log the
  pre-bind config value, always showing port 0 with the common `host:0`
  pattern, until that was found and fixed in `apps/stun-server/src/main.rs`).
  The reflector task spawns *before* this line logs, so waiting for `bind=`
  guarantees the reflector is already up too if you configured one.

### Env vars each binary reads (from their own `main.rs`/`config_from_env`)

`swarm-stun-server`: `SWARM_DATABASE_PATH`, `SWARM_HTTP_BIND` (use
`127.0.0.1:0` for an ephemeral port), `SWARM_REFLECTOR_PORTS`
(comma-separated; empty string disables the reflector entirely — do this
when a test doesn't need it), `SWARM_PUBLIC_URL`.

`swarm-serverd`: `SWARM_MEDIA_ROOT` (required), `SWARM_DATA_DIR`,
`SWARM_PEER_BIND`, `SWARM_ALLOW_FPS`, `SWARM_TOKEN_STORE_FILE_ONLY=1`
(always set this in tests — never touch the real OS keyring),
`SWARM_STUN_URL` + `SWARM_STUN_CODE` (both set together to
auto-register on startup, headless-deployment style — see
`apps/server/src/main.rs`), `SWARM_DEVICE_NAME`.

### Picking a free port without a real bind-then-immediately-close race concern

```kotlin
private fun freeUdpPort(): Int {
    DatagramSocket(0).use { return it.localPort }
}
```
Tiny race between closing this probe socket and the real subprocess
binding it — accepted throughout this repo's tests as reliable enough in
practice, not worth avoiding with more complexity.

## Web API driving (account/swarm/join-code creation)

There's no reusable Kotlin or Rust helper for the *browser* (cookie
session) side of the STUN API — every test hand-rolls a small `Browser`
class scoped to that one test file (register → login, capture
`swarm_session`/`swarm_csrf` cookies from `Set-Cookie`, then
`POST /api/v1/swarms`, `POST /api/v1/swarms/{id}/codes`). This is
deliberate duplication, not an oversight — see `swarm-verify-before-commit`'s
note on this repo's tolerance for small per-file test duplication over
premature shared test utilities. Copy the `Browser` class from the nearest
existing interop test file rather than trying to import one.

## Timing-sensitive gotchas specific to this pattern

- **TLS 1.3 rejection timing**: a client whose certificate gets rejected
  can still have `connect()` succeed — rejection often only surfaces on
  the *first request* over that connection. Always check "connect failed
  OR first request failed", never connect-time alone, when testing
  rejection.
- **Roster sync timing**: `register_with_stun`/`ServerCore` only
  syncs its roster once synchronously during registration, then every 30s.
  In a same-process Rust test you can call `.resync()` directly to force
  it; across a process boundary (Kotlin driving a real subprocess, or two
  separate subprocesses) there's no handle to do that — order operations
  so anyone who needs to be in another party's roster registers *before*
  that party's one guaranteed sync happens, not after.
- **quinn `Connection` lifecycle**: dropping a `Connection` handle does
  *not* send `CONNECTION_CLOSE` — the peer will sit waiting on
  `connection.closed()` until the idle timeout (tens of seconds) unless
  you call `connection.close(0u32.into(), b"...")` explicitly before
  the connection goes out of scope.
