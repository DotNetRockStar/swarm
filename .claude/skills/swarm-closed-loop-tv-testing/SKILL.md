---
name: swarm-closed-loop-tv-testing
description: Use when running, extending, or explaining scripts/tv_e2e_suite.sh — the automated closed-loop suite that tests the real desktop media server against real Amazon Fire TV(s) on the same LAN and fans out across every TV found, filing its findings to GitHub as an issue. Covers what "closed loop" means here, why each test case is evidence-based rather than UI-driven, and how fan-out discovery works. Change policy for this suite lives in swarm-e2e-suite-lockdown — read that before editing test logic.
---

# Closed-loop real-hardware testing: local media server <-> real Fire TV(s)

`./scripts/tv_e2e_suite.sh` is the automated version of the manual workflow
in `swarm-local-testing`: it builds the debug client, installs it on every
real Amazon Fire TV it can find on the LAN, and checks — with real evidence,
not a mock — that each one actually talks to the real local media server.
Its change policy is a separate, explicit skill:
**read `swarm-e2e-suite-lockdown` before editing it.**

## What "closed loop" means here

Every assertion is something a real device or a real server actually
reported, following the same evidence discipline as
`continuous-feedback-debugging` ("a hypothesis formed before looking at real
evidence is a guess"):

- **`install_and_launch`** — real `adb install` (in place, no wipe — see
  `deploy_fire_tv.sh`'s comment on why an unconditional uninstall is wrong),
  real `am start`, and the same 16s crash-poll `deploy_fire_tv.sh` uses,
  because a real crash on real hardware took measurably longer than a naive
  short sleep to surface (see `swarm-real-device-debugging`).
- **`lan_closed_loop_catalog`** — after launch, the suite sends **no D-pad
  input at all** and instead waits up to 20s for the client's own automatic
  startup reconnect (`SwarmViewModel.restoreSession()` → `openCatalog()`) to
  either succeed or fail on its own, then reads the result straight from
  logcat:
  - `browseCatalog() refresh done: entries=N unreachable=M` — the server
    answered and the client parsed a real manifest. `unreachable=0` is a
    PASS; a nonzero `unreachable` alongside this line is a FAIL (the
    round-trip started but the server side of it didn't complete).
  - `reconnect to already-paired LAN server ... failed` / `could not probe
    the saved LAN server during startup` — the client tried and the server
    didn't answer: FAIL.
  - Neither line appears — this TV has no saved LAN connection to reconnect
    to. That's a SKIP, not a FAIL: exercising first-time pairing would mean
    driving the passcode/D-pad UI blind, which `swarm-real-device-debugging`
    documents as carrying real risk (a stray `BACK` on this app's root
    screen has landed on a live Prime Video subscription checkout screen on
    real hardware). Pair the TV once by hand via the media server's Swarm
    page, and subsequent runs pick it up automatically.

This is why entries/unreachable counts are recorded as evidence in the
report rather than used as a hard pass bar on their own — an intentionally
empty media library legitimately produces `entries=0` without being a bug;
what proves the loop closed is the log line appearing at all.

## Fan-out discovery

No manual device selection — this suite is meant to run unattended. Target
selection, in order:

1. Explicit IP(s) passed as arguments.
2. Every already-`adb`-connected device reporting `ro.product.manufacturer`
   containing "Amazon".
3. Otherwise, a LAN ping/nc scan for port 5555, same technique
   `deploy_fire_tv.sh` uses (intentionally duplicated, not shared, so this
   frozen suite's discovery can't change out from under it if that script is
   edited later).

**Known sharp edge, already hit and fixed once:** any `while read` loop
whose input comes from a pipe (`<<<`, `< <(...)`) and whose body calls
`adb ... shell ...` must redirect that call's stdin from `/dev/null`.
`adb shell` otherwise reads from the loop's own input stream, so the first
device's shell call silently consumes every remaining line and only device
#1 is ever found — reproduced live against 3 real Fire TVs (a scan that
found all 3 candidates still only carried 1 through to the manufacturer
check), and present identically in `deploy_fire_tv.sh`'s LAN-scan loop until
fixed in the same pass. If a future discovery loop is added, give it the
same `</dev/null` treatment on every `adb shell` call from the start rather
than rediscovering this by watching devices silently disappear.

Every test case is run against every discovered device — the suite reports
one row per device per test case, not a single aggregate result, because a
regression that only reproduces on one Fire OS version/model is exactly the
kind of bug `swarm-real-device-debugging` documents this project's other
tooling can't catch any other way.

## Reporting findings to GitHub

Every run compiles a Markdown table (device, test, PASS/FAIL/SKIP, one-line
evidence) and files it as an issue via `gh issue create` (label `Testing`,
repo from `SWARM_GITHUB_REPOSITORY`, default `DotNetRockStar/swarm`) unless
run with `--no-issue`. Full per-device logcat captures are kept locally
under `.run/tv-e2e-reports/<timestamp>/` (gitignored, not attached to the
issue) so a human can pull the raw log for anything the one-line evidence
doesn't fully explain. If a run's own results are found to be wrong after
the fact (as happened once — see the fan-out fix above), correct it with a
follow-up comment on the same issue and, if the earlier title's totals are
now misleading, `gh issue edit --title`; don't leave a known-wrong report as
the only artifact.

## Preconditions this suite deliberately does not automate

- The local media server must already be running (`./scripts/run_now.sh`) —
  this suite only health-checks `http://127.0.0.1:$SWARM_STUN_PORT/health`
  and never starts/stops/restarts it, since it's GUI-owned and may already
  be serving real traffic (see `swarm-local-testing`).
- Each Fire TV must have ADB debugging enabled and already be
  authorized/paired at least once by a human, for the same reasons
  `deploy_fire_tv.sh` and `swarm-real-device-debugging` document.
