---
name: swarm-media-server-uat-tests
description: Use when creating, running, extending, debugging, or explaining the media server's backend UAT suite — apps/server/src/gui_tests/ and scripts/tests/media_server_uat_tests.sh. Covers the direct-command-invocation test shape (no simulated Tauri IPC/ACL, no real UI), per-test data/port isolation, the ServerCore startup-scan race and how the harness settles it, and why this suite is backend/API-only. Read swarm-e2e-suite-lockdown before editing test logic.
---

# Media server backend UAT suite

`./scripts/tests/media_server_uat_tests.sh` (a thin wrapper around `cargo test -p
swarm-server --bin swarm-server-app --features gui`) runs real
`#[tauri::command]` handlers from `apps/server/src/gui.rs` directly against a
real, isolated `AppState` — real SQLite (`library.sqlite`), real filesystem,
a real `ServerCore` with its own unique QUIC/HTTP ports per test — behind
`tauri::test`'s mocked runtime. No real Fire TV, no LAN, no already-running
desktop server: it's plain `cargo test`, runs in about a second, and is
CI-friendly. Its change policy is the same explicit, standing user rule as
the two TV suites — **read `swarm-e2e-suite-lockdown` before editing
anything under `apps/server/src/gui_tests/`.**

## Why this shape, not the alternatives already tried

Two other approaches were evaluated and rejected before this one, in order:

1. **Real native UI automation.** Tauri's official WebDriver story
   (`tauri-driver`) has no macOS backend. A macOS Accessibility-API spike
   (AppleScript/System Events driving the real running app) proved
   unreliable enough to not be worth pursuing further: `click` via `AXPress`
   didn't reliably reach the WKWebView's real DOM click handlers, and full
   accessibility-tree enumeration was non-deterministic against a window
   with a large, frequently-refreshing list (identical queries sometimes
   found an element, sometimes didn't, with nothing actually changing in the
   app). The user chose to proceed with backend/API-only coverage instead of
   continuing to fight this.
2. **Tauri's simulated IPC (`tauri::test::get_ipc_response`).** This is the
   "intended" way to test Tauri commands, but Tauri 2's ACL/capability
   manifest for app-level (non-plugin) commands doesn't exist under a bare
   `mock_context()` — every invoke fails with `UnknownManifest { key: "app
   manifest", ... }`, a build-time-generated artifact this app's tests have
   no reasonable way to construct. Reconstructing it would be testing Tauri
   framework plumbing, not this app's logic.

What actually works, and what this suite uses: call the `async fn` command
handlers directly (they're plain Rust functions annotated
`#[tauri::command]`, callable like any other function) with a real
`AppHandle<MockRuntime>` obtained from `app.handle().clone()` — bypassing
Tauri's IPC/ACL layer entirely while still exercising 100% of the real
command body, the real `AppState`, and the real `ServerCore`. This is a
deliberate, permanent scoping decision, not a stopgap: it means real
user-visible UI flows (browse, playback, watchlist, ...) are **not** covered
here — that coverage lives in `swarm-tv-uat-suite`'s real-device suite
(server-side effects only) or needs a human, until a reliable macOS UI
automation path exists.

## Per-test isolation (why `AppState` grew two test-only fields)

Every command function takes `tauri::AppHandle`, which Tauri normally
resolves to the concrete `Wry` runtime. To run under `tauri::test`'s
`MockRuntime` instead, every command's `AppHandle` parameter is genericized
to `AppHandle<R: tauri::Runtime>` — mechanical, no behavior change, verified
by a full workspace rebuild plus the full pre-existing test suite passing.

Two fields on `AppState` exist only for this suite, always `None` in
production:

- **`test_data_dir: Option<PathBuf>`** — checked first inside `app_data_dir()`
  before falling back to Tauri's real `app.path().app_data_dir()`.
  `mock_context()`'s identifier defaults to empty, so without this every
  test would resolve to the same shared OS path and corrupt each other's
  `settings.json`/`library.sqlite` when tests run concurrently.
- **`test_bind_override: Option<(SocketAddr, SocketAddr)>`** — checked first
  inside `AppState::core()` before falling back to the
  `SWARM_PEER_BIND`/`SWARM_HTTP_MEDIA_BIND` env-var overrides (which default
  to SWARM's real ports, 8543/8546). Env vars are process-global, so two
  tests that each start a real `ServerCore` concurrently would otherwise
  race to bind the same ports. `harness.rs`'s `next_port_pair()` hands out a
  unique pair per test from a shared atomic counter starting at 23000 — far
  from SWARM's real defaults, so a concurrently-running dev instance of the
  app is never affected by a test run.

This is why Rust's default parallel `#[test]` execution is safe here with no
`--test-threads=1` workaround: every test gets a genuinely private data
directory and network ports, not just a private in-memory struct.

## The `ServerCore` startup-scan race (read before writing a new test)

`ServerCore::start()` spawns its initial library scan in the background
rather than awaiting it (see its doc comment: a real user's library scan
must never block the first command after launch). This is correct product
behavior, but it means the *first* time a test's `AppState::core()` call
starts a real core, there are now two scans in flight that both want the
same internal `scan_lock`: that background startup scan, and the test's own
explicit `rescan()` call. Which one wins the race for the lock is
non-deterministic — under Rust's default single-threaded `#[tokio::test]`
runtime it usually depends on which task happens to reach the lock's
`.await` point first, and a test's own `rescan()` racing *ahead* of the
still-pending startup scan produces a result the startup scan then silently
overwrites moments later, after the test has already asserted on the wrong
one. This produced a real, intermittent-looking test failure during this
suite's own development (a deleted file's removal wasn't reflected in the
`rescan()` result the test checked, even though a background log line
moments later showed the correct reconciliation happening).

The fix lives in `harness.rs`'s `test_app_with_media_root()`: after adding
the media root, it explicitly initializes the core and awaits
`ServerCore::wait_for_scan()` — a method the product code already exposes
specifically for this ("mainly for tests that need deterministic post-scan
assertions," per its own doc comment) — before returning control to the
test. Every test built on top of `test_app_with_media_root()` therefore
starts from a settled core with no scan in flight, and its own subsequent
`rescan()` calls are the only scan running. **Any new test that starts a
real `ServerCore` must go through `test_app_with_media_root()` (or otherwise
await `wait_for_scan()` itself) before making its own scan-related
assertions** — skipping this reintroduces the same race.

## Scenario catalog

One file per category under `apps/server/src/gui_tests/`:

| File | Covers |
|---|---|
| `media_root_lifecycle.rs` | `add_media_root`/`list_media_roots`/`remove_media_root`: persistence, duplicate-label rejection, the last-root-removal guard. Settings-file only — never starts a `ServerCore`. |
| `library_scan.rs` | `rescan`/`list_entries`: add/update/remove reconciliation against real files on disk, idempotent no-op rescans, and the real "found 0 files but the library already has entries" safety guard that refuses to treat a fully-emptied root as mass deletion (a dropped network mount looks identical to a genuinely empty one). |
| `tv_pairing.rs` | `approve_lan_pairing`'s real invalid/expired-code error path; `list_local_peers` on a fresh core. A real pairing handshake needs a real TV on the LAN — that's `swarm-tv-uat-suite`'s job, not this suite's. |
| `notifications_and_errors.rs` | `list_client_errors`/`resolve_client_error`/`clear_client_errors`, seeded through `Library::record_client_error` — the same write path a real client's error report takes over the wire, just without needing a real TV to send one. |
| `metadata_editing.rs` | `set_manual_metadata` overriding title/genres/overview/rating on a real scanned entry, and its real (surprising) no-op-not-error behavior on an unknown entry key — see below. |
| `mcp_tokens.rs` | `generate_mcp_access_token` (persistence, rotation to a fresh value each call) and `set_mcp_enabled`'s toggle persistence. |

Not covered, deliberately: subtitle generation/transcription (needs a real
audio/video fixture plus `ffmpeg`/Whisper — a materially heavier dependency
than everything else here) and SWARM/STUN membership commands (need a real
or faked rendezvous server). Flag to the user before adding either — they
weren't in-scope for this pass, not ruled out for a future one.

## Known real-behavior surprises, confirmed while writing these tests

Two assertions this suite's own first draft got wrong, both because the
actual product behavior was more specific than assumed — kept here so a
future test doesn't repeat the same wrong assumption:

- **`entry.title` vs `entry.scraped_title`.** `set_manual_metadata`'s title
  override lands in `scraped_title` (the scraper/manual-override display
  field), never in `title` (the path-derived grouping key — see
  `classify.rs`'s "grouping keys are always path/filename-derived"
  invariant). Assert `scraped_title` for a manual title edit, not `title`.
- **`set_manual_metadata` on an unknown `entry_key` does not error.**
  `Library::set_manual_metadata` is a plain `UPDATE ... WHERE entry_key = ?`
  with no existence check; zero rows affected still returns `Ok(())`. If
  this should actually be a real validation gap, that's a product decision
  for the user to make deliberately — this suite documents the current
  behavior rather than asserting a stricter contract the command doesn't
  implement.

## Creating or changing a test

First read `swarm-e2e-suite-lockdown`. A failing test is not permission to
change its assertion — only a user's explicit request in the current
conversation changes a frozen scenario's contract. When a test's assertion
turns out to be based on a wrong assumption about product behavior (as
happened twice during this suite's own first pass — see above), fix the
*test* to match verified real behavior; that's a correction, not a scope
change, and doesn't need to wait for permission. Then:

1. Decide whether the new coverage needs a real `ServerCore`
   (`test_app_with_media_root()`) or is settings-file-only (`test_app()`).
2. If it calls a command function that doesn't yet take a generic
   `AppHandle<R: tauri::Runtime>`, genericize its `AppHandle` parameter —
   mechanical, verify with `cargo build --workspace` plus this suite
   afterward.
3. Seed real state through the same write paths production code uses
   (`Library::record_client_error`, a real file written to a real temp
   media-root directory, ...), never by hand-constructing a row that
   bypasses the real command/store logic being tested.
4. Add the test to the relevant category file, or a new file registered in
   `gui_tests/mod.rs` for a genuinely new category.
5. Run `./scripts/tests/media_server_uat_tests.sh`, and run it a few times in a
   row — this suite's own port/data isolation should make it fully
   deterministic; repeated runs are how the startup-scan race above was
   originally caught.
6. Document the new file/category in `scripts/tests/TV_TESTING.md`'s scenario
   table.

## Invocation

```bash
./scripts/tests/media_server_uat_tests.sh              # run every backend UAT test
./scripts/tests/media_server_uat_tests.sh media_root    # run only tests whose name contains this substring
```

Exit code `0` only if every test passed. Nothing here calls an LLM at run
time — a human, CI, or an AI agent all invoke the exact same deterministic
`cargo test` run.
