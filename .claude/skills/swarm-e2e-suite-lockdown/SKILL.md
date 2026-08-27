---
name: swarm-e2e-suite-lockdown
description: Use before touching scripts/tv_e2e_suite.sh OR scripts/tv_uat_suite.sh (the two closed-loop real-Fire-TV suites) for any reason, or when a run of either reports a FAIL/SKIP and the instinct is to "fix the test." States the user's standing rule that neither suite's test logic may change without their explicit, in-conversation permission — an AI agent finding it inconvenient is not permission.
---

# These test suites are frozen by explicit user policy

The user who requested `scripts/tv_e2e_suite.sh` (closed-loop testing of the
real desktop media server against real Amazon Fire TV hardware, see
`swarm-closed-loop-tv-testing`) was explicit: **the suite must stay
consistent, and neither Claude nor Codex may change it on their own
initiative.** This is a standing rule for every future session that touches
this repo, not a one-time instruction that expired when the suite first
shipped. When the user later asked for a second, much larger UI-driving UAT
suite (`scripts/tv_uat_suite.sh` + the instrumented sources under
`clients/tv-android/app/src/androidTest/kotlin/app/swarm/tv/app/uat/`, see
`swarm-tv-uat-suite`), they explicitly extended the same rule to it. **Both
suites are frozen the same way, independently.**

## What is frozen

### `scripts/tv_e2e_suite.sh` (log-evidence only, no UI navigation)

- Which test cases run (`install_and_launch`, `lan_closed_loop_catalog`,
  `testing_mode_cleanup`, and any later addition made under explicit user
  direction).
- Their pass/fail/skip thresholds and the log lines/exit codes they key on.
- The device-targeting precedence: explicit `--device`/positional target(s)
  > preferred device from `scripts/tv_test_device.local.json` (gitignored,
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

### `scripts/tv_uat_suite.sh` + the `uat` instrumented test sources

- Which scenario classes/methods run (see `swarm-tv-uat-suite` for the full
  catalog) and what each one asserts — UI state via Compose test tags, TV-side
  persisted state (Room `swarm.db` and the liked/watchlist/watch-state
  `SharedPreferences` stores) read in-process, and server-side SQLite state
  (`library_entries`, `entry_likes`, `client_errors`, `server_notifications`
  in `library.sqlite`) queried from the host.
- The device-targeting precedence (explicit `--device` > preferred device
  from `scripts/tv_test_device.local.json` > already-connected Amazon devices > full
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

## What is NOT frozen

- **The product code either suite exercises.** A FAIL means something in
  `apps/server`, `crates/`, or `clients/tv-android` is broken — go fix that,
  the same as any other bug report, and let the suite re-confirm it. The
  UAT suite's evidence bundle exists specifically so this fix can happen
  without re-running the suite just to see what broke.
- **Genuine infrastructure bugs in either suite's own supporting logic** (a
  shell quoting bug, a stdin-consumption bug in a discovery loop, an adb
  timing race, an instrumentation-output parsing edge case) — these are
  implementation bugs, not test-policy changes, and fixing them so the suite
  behaves as designed is expected maintenance, not a rule violation. The
  bar: does the fix change *what* the suite verifies or *how strictly*, or
  does it just make the existing verification actually execute correctly?
  Only the latter is fair game without asking.
- Adding a brand-new, clearly-separate script (or a brand-new scenario in
  either suite) that tests something not yet covered — under explicit user
  direction, same as any other change here.

## The failure mode this skill exists to prevent

An agent picks up a follow-up issue, runs one of the suites, sees a real
FAIL, and — under time pressure to "complete the issue" — edits the
assertion, loosens its threshold, or removes the failing test case (or, in
the UAT suite, quietly drops a piece of the failure-evidence bundle) so the
run goes green. That produces a false all-clear on real hardware and is
exactly the outcome the user asked to be protected from. If a test result
looks wrong (flaky timing, a device-specific quirk), say so in the findings
report and ask the user in the next planning step — do not quietly adjust
either suite to stop noticing it.

## If a change to either suite is genuinely warranted

Only make it when the user explicitly asks for it in the current
conversation (e.g., "add a seek test," "raise the catalog-refresh timeout to
30s," "also test pause/resume," "add a scenario for X"). Implement exactly
what was asked, note the change plainly in the commit message, and don't
bundle it with an unrelated product-code fix in the same commit — a reviewer
re-reading history later should be able to see "the suite's contract changed
here, on request" as a single, obvious commit.
