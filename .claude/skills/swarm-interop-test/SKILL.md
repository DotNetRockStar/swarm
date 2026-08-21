---
name: swarm-interop-test
description: Use when writing integration tests across the SWARM service, native GUI-owned ServerCore, Rust QUIC stack, or Kotlin TV client. Covers in-process Rust fixtures, real subprocess boundaries, readiness/cleanup, timing traps, and the removed headless media-server fixture.
---

# Testing real SWARM interoperability

Prefer real protocol implementations over mocks, but use the boundary that
matches the production architecture. The desktop media server is the native GUI
owning one `ServerCore`; there is no production `swarm-serverd` binary.

## Rust-to-Rust

Run the SWARM Axum router in process with `TcpListener("127.0.0.1:0")` and
`axum::serve` in a spawned task. Start media behavior directly with
`ServerCore::start(...)` and a temporary data/media directory. This exercises
the same server core used by the GUI without needing to automate a Tauri
window. Add `stun-server` as a dev dependency where required.

Use real readiness, not fixed sleeps. A bound listener is ready before its task
is spawned; for asynchronous registration, poll the API/state with a deadline.
Keep task handles and abort them in teardown. Temporary identities, token
stores, databases, and media files must stay in test-owned temp directories.

## Kotlin-to-Rust

`:core:interopTest` may spawn `swarm-stun-server` because that is still a real
standalone service. Build its release binary first and locate it from the
module's stable relative path. Start with `ProcessBuilder`, set test-only env
vars, read stdout until its resolved `bind=<address>` readiness line, then
drain the remaining pipe on a daemon thread. Track every process and destroy it
forcibly with a bounded wait in `@AfterEach`.

Legacy Kotlin tests still search for the removed release
`target/release/swarm-serverd` and therefore skip. A skipped test is not proof
of current media-server interoperability. Do not restore a production headless
binary merely to make those fixtures run. When cross-language media-peer
coverage is required, add an explicitly test-only Rust harness that owns
`ServerCore`, or drive the packaged GUI in a separately scoped end-to-end test.

## Standalone service subprocess pattern

Configure `swarm-stun-server` with test-owned values:

- `SWARM_DATABASE_PATH`
- `SWARM_HTTP_BIND=127.0.0.1:0`
- `SWARM_REFLECTOR_PORTS` (empty when reflection is not needed)
- `SWARM_PUBLIC_URL`
- `RUST_LOG=info`

Wait for the resolved `bind=(IPv4:port)` log line. The reflector starts before
that line, so it is also ready. Strip ANSI escape sequences defensively and
keep draining stdout so the child cannot block on a full pipe.

For UDP reflector ports, probing with `DatagramSocket(0)` then closing it has a
small accepted race. HTTP listeners should bind port zero directly whenever
the API permits it.

## Timing and lifecycle traps

- TLS 1.3 certificate rejection can surface on the first request rather than
  `connect()`. Accept failure at either boundary in rejection tests.
- Dropping a Quinn `Connection` does not guarantee an immediate close; call
  `connection.close(...)` so the peer does not wait for idle timeout.
- Roster synchronization is asynchronous. In Rust tests call `.resync()` when
  the test owns the handle; across processes order registrations before the
  consumer's guaranteed initial sync or poll with a deadline.
- Re-run new timing-sensitive tests at least three times.
- Build release subprocesses immediately before tests so a green run cannot be
  against a stale binary.

Browser-style account/session API tests may keep a small per-file helper that
captures session and CSRF cookies. Prefer local explicit duplication over a
global abstraction that obscures which HTTP calls a protocol test performs.
