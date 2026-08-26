---
name: swarm-e2e-suite-lockdown
description: Use before touching scripts/tv_e2e_suite.sh (the closed-loop real-Fire-TV E2E suite) for any reason, or when a run of it reports a FAIL/SKIP and the instinct is to "fix the test." States the user's standing rule that this suite's test logic must not change without their explicit, in-conversation permission — an AI agent finding it inconvenient is not permission.
---

# This test suite is frozen by explicit user policy

The user who requested `scripts/tv_e2e_suite.sh` (closed-loop testing of the
real desktop media server against real Amazon Fire TV hardware, see
`swarm-closed-loop-tv-testing`) was explicit: **the suite must stay
consistent, and neither Claude nor Codex may change it on their own
initiative.** This is a standing rule for every future session that touches
this repo, not a one-time instruction that expired when the suite first
shipped.

## What is frozen

In `scripts/tv_e2e_suite.sh`:

- Which test cases run (`install_and_launch`, `lan_closed_loop_catalog`,
  `testing_mode_cleanup`, and any later addition made under explicit user
  direction).
- Their pass/fail/skip thresholds and the log lines/exit codes they key on.
- The fan-out behavior (discover every reachable Amazon Fire TV, run every
  test case against each one, never silently narrow to a subset).
- That it never starts, stops, or restarts the media server itself (GUI-owned
  lifecycle — see `swarm-local-testing`) and never drives first-time D-pad
  pairing (real navigation risk on a device with a real Amazon account behind
  it — see `swarm-real-device-debugging`). First-time pairing is instead
  exercised through the explicitly-authorized debug testing mode added for
  issue #81: isolated certificate, 10-minute maximum, no persistence, and an
  adb/control-file secret in addition to visible code `00000000`.
- That every run compiles and files its findings to GitHub as an issue (see
  `swarm-closed-loop-tv-testing`), rather than only printing to a terminal.

## What is NOT frozen

- **The product code the suite exercises.** A FAIL means something in
  `apps/server`, `crates/`, or `clients/tv-android` is broken — go fix that,
  the same as any other bug report, and let the suite re-confirm it.
- **Genuine infrastructure bugs in the suite's own supporting logic** (a
  shell quoting bug, a stdin-consumption bug in a discovery loop, an adb
  timing race) — these are implementation bugs, not test-policy changes, and
  fixing them so the suite behaves as designed is expected maintenance, not
  a rule violation. The bar: does the fix change *what* the suite verifies or
  *how strictly*, or does it just make the existing verification actually
  execute correctly? Only the latter is fair game without asking.
- Adding a brand-new, clearly-separate script that tests something this
  suite doesn't yet cover.

## The failure mode this skill exists to prevent

An agent picks up a follow-up issue, runs the suite, sees a real FAIL, and —
under time pressure to "complete the issue" — edits the suite's assertion,
loosens its threshold, or removes the failing test case so the run goes
green. That produces a false all-clear on real hardware and is exactly the
outcome the user asked to be protected from. If a test result looks wrong
(flaky timing, a device-specific quirk), say so in the findings report and
ask the user in the next planning step — do not quietly adjust the suite to
stop noticing it.

## If a change to the suite is genuinely warranted

Only make it when the user explicitly asks for it in the current
conversation (e.g., "add a seek test," "raise the catalog-refresh timeout to
30s," "also test pause/resume"). Implement exactly what was asked, note the
change plainly in the commit message, and don't bundle it with an unrelated
product-code fix in the same commit — a reviewer re-reading history later
should be able to see "the suite's contract changed here, on request" as a
single, obvious commit.
