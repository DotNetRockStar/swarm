---
name: swarm-e2e-suite-lockdown
description: Use before touching scripts/tests/tv_e2e_suite.sh, scripts/tests/tv_uat_suite.sh, scripts/tests/media_server_uat_tests.sh, scripts/tests/full_uat_suite.sh, OR scripts/tests/full_uat_cron.sh (the three closed-loop suites — two real-Fire-TV, one backend-only — plus the orchestrator that runs all three and the continuous-checking loop around that) for any reason, or when a run of any of them reports a FAIL/SKIP and the instinct is to "fix the test." States the user's standing rule that none of their test logic may change without their explicit, in-conversation permission — an AI agent finding it inconvenient is not permission.
---

# These test suites are frozen by explicit user policy

The user who requested `scripts/tests/tv_e2e_suite.sh` (closed-loop testing of the
real desktop media server against real Amazon Fire TV hardware, see
`swarm-closed-loop-tv-testing`) was explicit: **the suite must stay
consistent, and neither Claude nor Codex may change it on their own
initiative.** This is a standing rule for every future session that touches
this repo, not a one-time instruction that expired when the suite first
shipped. When the user later asked for a second, much larger UI-driving UAT
suite (`scripts/tests/tv_uat_suite.sh` + the instrumented sources under
`clients/tv-android/app/src/androidTest/kotlin/app/swarm/tv/app/uat/`, see
`swarm-tv-uat-suite`), they explicitly extended the same rule to it. The same
rule applies again to the third suite, `scripts/tests/media_server_uat_tests.sh` +
`apps/server/src/gui_tests/` (real `#[tauri::command]` handlers invoked
directly against a real, isolated backend — no hardware, no UI — see
`swarm-media-server-uat-tests`), added when the user chose backend/API-only
coverage after two real-UI-automation approaches proved unreliable, and again
to `scripts/tests/full_uat_suite.sh`, the orchestrator the user asked for to run
all three suites together, gather each one's evidence, and file a single
consolidated GitHub issue only when at least one test actually failed, and
once more to `scripts/tests/full_uat_cron.sh`, the continuous-checking loop
the user asked for around that orchestrator — re-running it whenever `main`
changes and tracking failures/fixes in one reused GitHub issue instead of a
fresh one every run. **All five are frozen the same way, independently.**

## What is frozen

### `scripts/tests/tv_e2e_suite.sh` (log-evidence only, no UI navigation)

- Which test cases run (`install_and_launch`, `lan_closed_loop_catalog`,
  `testing_mode_cleanup`, and any later addition made under explicit user
  direction).
- Their pass/fail/skip thresholds and the log lines/exit codes they key on.
- The device-targeting precedence: explicit `--device`/positional target(s)
  > preferred device from `scripts/tests/tv_test_device.local.json` (gitignored,
  shared with `tv_uat_suite.sh`) > every already-adb-connected Amazon device
  > full LAN scan fan-out; `--all` always forces full fan-out regardless of
  the preference file. Defaulting to a configured preferred device (added
  under explicit user direction, alongside the same change to
  `tv_uat_suite.sh`) is itself part of the frozen contract now — every test
  case still runs against every device actually selected, never a silent
  further narrowing beyond that selection.
- That it never starts, stops, or restarts the media server itself (GUI-owned
  lifecycle — see `swarm-local-testing`) and never drives first-time D-pad
  pairing (real navigation risk on a device with a real Amazon account behind
  it — see `swarm-real-device-debugging`). First-time pairing is instead
  exercised through the explicitly-authorized debug testing mode added for
  issue #81: isolated certificate, 10-minute maximum, no persistence, and an
  adb/control-file secret in addition to visible code `00000000`.
- That every run compiles a findings report, and files it to GitHub as an
  issue (see `swarm-closed-loop-tv-testing`) whenever at least one test
  failed — changed from "every run, pass or fail" to "only on a real
  failure" under explicit user direction; a clean PASS run still writes the
  local report, just never opens an issue for it.

### `scripts/tests/tv_uat_suite.sh` + the `uat` instrumented test sources

- Which scenario classes/methods run (see `swarm-tv-uat-suite` for the full
  catalog) and what each one asserts — UI state via Compose test tags, TV-side
  persisted state (Room `swarm.db` and the liked/watchlist/watch-state
  `SharedPreferences` stores) read in-process, and server-side SQLite state
  (`library_entries`, `entry_likes`, `client_errors`, `server_notifications`
  in `library.sqlite`) queried from the host.
- The device-targeting precedence (explicit `--device` > preferred device
  from `scripts/tests/tv_test_device.local.json` > already-connected Amazon devices > full
  LAN fan-out) and that `--all` always means every discovered device, never
  a silently-narrowed subset.
- The failure-evidence bundle: every FAIL must produce the full TV-to-server
  dump (screenshot, window-hierarchy XML, Compose semantics tree, on-device
  logcat, `swarm.db` + SharedPreferences pulls, server SQLite rows, and the
  server log tail) under `.run/tv-uat-reports/<run>/<device>/<test>/` — do
  not narrow or drop pieces of this bundle to make failures less visible.
- That it never starts/stops the media server and never drives first-time
  D-pad pairing, for the same reasons as the original suite.
- That every run compiles a findings report, filed to GitHub as an issue
  only on failure — same as the original suite.

### `scripts/tests/media_server_uat_tests.sh` + `apps/server/src/gui_tests/`

- Which category files/tests run (see `swarm-media-server-uat-tests` for the
  full catalog: media root lifecycle, library scan, TV pairing, notifications/
  client-errors, metadata editing, MCP tokens) and what each one asserts —
  real command-handler return values and real SQLite/settings-file state
  read back afterward, not a mocked layer.
- The direct-command-invocation test shape itself (real `AppHandle<MockRuntime>`,
  real `AppState`, commands called as plain functions) rather than Tauri's
  simulated IPC/ACL layer — this isn't an implementation detail, it's the
  deliberate result of the two rejected approaches documented in
  `swarm-media-server-uat-tests`; reverting to `get_ipc_response` or
  attempting real native UI automation without the user explicitly asking to
  revisit that decision is a policy change, not a refactor.
- The per-test data-dir/port isolation contract (`AppState::test_data_dir`/
  `test_bind_override`, allocated per test so the suite runs safely under
  Rust's default parallel test execution) — do not collapse this to a shared
  fixture or add `--test-threads=1` to work around a collision instead of
  preserving isolation.
- That every new test needing a real `ServerCore` goes through
  `test_app_with_media_root()` (or otherwise awaits `wait_for_scan()`) so it
  never races the startup background scan — see `swarm-media-server-uat-tests`
  for the real race this prevents.
- That this suite stays backend/API-only by design (no real UI drives it) —
  covering an actual user-visible UI flow belongs in `swarm-tv-uat-suite`
  (server-side effects) or awaits a reliable macOS UI-automation path, not a
  simulated substitute added here.

### `scripts/tests/full_uat_suite.sh` (orchestrator — runs all three above)

This wraps the three suites without changing any of them: it runs
`media_server_uat_tests.sh` → `tv_e2e_suite.sh` → `tv_uat_suite.sh` in
order, always suppressing each wrapped suite's own issue-filing
(`--no-issue`), and files at most one consolidated GitHub issue itself.
Frozen about the orchestrator specifically:

- The default set it runs (all three) and that `--skip-backend`/
  `--skip-e2e`/`--skip-uat`/`--include-resilience` are opt-out/opt-in, not a
  silently narrowed default — don't drop a suite from the default run to
  make a flaky day go quiet.
- That it never reimplements a suite's own evidence-gathering — it captures
  each suite's full console output and folds in whatever report file that
  suite already wrote, nothing more; evidence-bundle *content* stays each
  suite's own frozen responsibility (see their sections above).
- That it files a consolidated issue only when `--github-issue` was passed
  **and** `TOTAL_FAIL > 0` — both conditions, same "only on a real failure"
  policy as the suites it wraps — and that the issue body includes every
  suite's result, passes and failures both, not just the failing ones.

### `scripts/tests/full_uat_cron.sh` (continuous checking around the orchestrator)

A foreground, Ctrl+C-able loop (deliberately not a real system cron/
launchd job) that re-runs `full_uat_suite.sh` only when the locally
checked-out `main` commit has actually changed since the last check
(covers both new commits landed and local commits not yet pushed), never
overlapping a still-in-progress run (a PID-liveness-checked lock file), and
reuses one tracking GitHub issue across runs — commenting failures and
recoveries onto it while it stays open, filing a fresh one only after the
previous one was closed — instead of filing a new issue every run. Frozen
about this wrapper specifically:

- That it never mutates the working tree or pulls/merges anything on its
  own — it only `git fetch`es (to keep the not-yet-pushed count accurate)
  and tests whatever is already checked out locally, exactly as-is. Adding
  an automatic pull/merge/checkout here is a real behavior change, not a
  bugfix, even if it seems convenient.
- The "one open tracking issue, reused until closed" contract: a failure
  comments on the currently-open tracking issue (verified open via a live
  `gh issue view`, never assumed from stale state) if one exists, or files
  a new one if not; a pass comments a recovery note on an open tracking
  issue but never opens a new issue on its own.
- The lock-file overlap guard — never remove or weaken this to let two
  `full_uat_suite.sh` runs execute concurrently.

## What is NOT frozen

- **The product code any suite exercises.** A FAIL means something in
  `apps/server`, `crates/`, or `clients/tv-android` is broken — go fix that,
  the same as any other bug report, and let the suite re-confirm it. The
  UAT suites' evidence (the full TV-to-server bundle, or `cargo test`'s own
  panic output for the backend suite) exists specifically so this fix can
  happen without re-running the suite just to see what broke.
- **Genuine infrastructure bugs in any suite's own supporting logic** (a
  shell quoting bug, a stdin-consumption bug in a discovery loop, an adb
  timing race, an instrumentation-output parsing edge case, a test harness
  bug like the startup-scan race documented in
  `swarm-media-server-uat-tests`) — these are implementation bugs, not
  test-policy changes, and fixing them so the suite behaves as designed is
  expected maintenance, not a rule violation. The bar: does the fix change
  *what* the suite verifies or *how strictly*, or does it just make the
  existing verification actually execute correctly? Only the latter is fair
  game without asking. A test assertion built on a wrong assumption about
  real product behavior (confirmed by reading the actual implementation, not
  guessed) falls in this same fair-game category — fix the assertion to
  match verified real behavior, same as any other test bug.
- Adding a brand-new, clearly-separate script (or a brand-new scenario/
  category in any suite) that tests something not yet covered — under
  explicit user direction, same as any other change here.

## The failure mode this skill exists to prevent

An agent picks up a follow-up issue, runs one of the suites, sees a real
FAIL, and — under time pressure to "complete the issue" — edits the
assertion, loosens its threshold, or removes the failing test case (or, in
a UAT suite, quietly drops a piece of the failure-evidence bundle) so the
run goes green. That produces a false all-clear and is exactly the outcome
the user asked to be protected from. If a test result looks wrong (flaky
timing, a device-specific quirk), say so in the findings report and ask the
user in the next planning step — do not quietly adjust any suite to stop
noticing it.

## If a change to any suite is genuinely warranted

Only make it when the user explicitly asks for it in the current
conversation (e.g., "add a seek test," "raise the catalog-refresh timeout to
30s," "also test pause/resume," "add a scenario for X"). Implement exactly
what was asked, note the change plainly in the commit message, and don't
bundle it with an unrelated product-code fix in the same commit — a reviewer
re-reading history later should be able to see "the suite's contract changed
here, on request" as a single, obvious commit.
