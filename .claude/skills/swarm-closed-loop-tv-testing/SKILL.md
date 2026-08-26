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

## What's required to run this, and what's automated vs. manual

Everything below is either already automated by the suite or is a one-time
human step that can't be safely automated — there is no third category of
"manual today, could be scripted later but nobody's gotten to it," except
where noted.

**Automated by the suite, every run, no human involved:**

- Discovering every reachable Amazon Fire TV on the LAN (already-connected
  `adb` devices first, then a ping/nc scan) and fanning out across all of
  them — see "Fan-out discovery" above.
- Waking a discovered TV if it's asleep (`dumpsys power` shows
  `mWakefulness` other than `Awake` → a single `KEYCODE_WAKEUP` keyevent).
  This is just turning the screen on, not UI navigation, so it carries none
  of the real-hardware risk documented below for pairing.
- Building, installing (in place, no wipe), force-stopping, and launching
  the debug client on every discovered device.
- Waiting for and reading the automatic LAN-reconnect/catalog-refresh
  result from logcat, with no D-pad input sent.
- Compiling and filing the findings report to GitHub.

**Genuinely manual, one-time per TV, and not going to be automated:**

- **ADB debugging must be enabled on the TV, and the very first connection
  from this Mac must be authorized by tapping "Allow" on the TV's own
  on-screen trust prompt.** This is an Android security control, not a
  workflow gap — `adb` is designed so a computer can't grant itself
  debugging access to a device without a human physically confirming it on
  that device. There's no `adb` command that accepts this prompt from the
  connecting side. Do this once per TV (`deploy_fire_tv.sh` will also
  print this instruction the first time it hits an unauthorized device);
  every run after that reconnects automatically.
- **Each TV must be paired with the local media server once, by hand,
  through the app's own D-pad UI** (Swarm page → enter the passcode the
  media server's dashboard displays) before `lan_closed_loop_catalog` can
  produce PASS/FAIL instead of SKIP. The suite deliberately never drives
  this itself: it would mean sending blind D-pad input on a device signed
  into a real Amazon account, and a stray `BACK` on this app's root screen
  has landed on a live Prime Video subscription checkout screen on real
  hardware (see `swarm-real-device-debugging`). Until a TV is paired, its
  `lan_closed_loop_catalog` row will keep reporting SKIP — that's expected,
  not a bug, and is exactly what the evidence-based reporting above is
  designed to surface rather than hide.
- **The local media server must already be running** (`./scripts/run_now.sh`)
  before invoking the suite. This suite only health-checks
  `http://127.0.0.1:$SWARM_STUN_PORT/health` and never starts, stops, or
  restarts it, since the server's lifecycle is GUI-owned and it may already
  be serving real traffic (see `swarm-local-testing`). Starting the GUI
  itself isn't something an unattended script should do on someone's
  machine on its own initiative.
