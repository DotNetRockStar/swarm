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
  input at all**. It explicitly arms the debug client for a non-persistent
  10-minute testing session through already-authorized adb; the client uses
  an isolated testing certificate, discovers the real media server, performs
  the real LAN activation exchange, and opens the catalog. The suite then
  reads the result straight from logcat:
  - `browseCatalog() refresh done: entries=N unreachable=M` — the server
    answered and the client parsed a real manifest. `unreachable=0` is a
    PASS; a nonzero `unreachable` alongside this line is a FAIL (the
    round-trip started but the server side of it didn't complete).
  - `reconnect to already-paired LAN server ... failed` / `could not probe
    the saved LAN server during startup` — the client tried and the server
    didn't answer: FAIL.
  - Neither line appears within 40 seconds — FAIL: debug pairing or the real
    catalog round-trip did not complete. First-time pairing is no longer a
    manual prerequisite and is no longer reported as SKIP.
- **`testing_mode_cleanup`** — the suite disables testing mode through adb
  after the catalog assertion and requires the client's structured cleanup
  confirmation. That cleanup closes the QUIC route, deletes derived catalog
  cache, destroys the testing certificate, and explicitly asks the server to
  revoke the exact ephemeral grant.

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
  the debug client on every discovered device with testing mode explicitly
  armed for that process.
- Creating a high-entropy per-run automation token in the host-only
  `.run/tv-e2e-control.json` file. The visible code is deliberately fixed at
  `00000000`, but that code alone never authorizes unattended enrollment:
  release servers reject the testing action, and the debug server requires
  the secret token for automatic approval.
- Pairing an isolated, non-persistent testing certificate; waiting for the
  catalog refresh; then disabling the mode and verifying cleanup from logcat.
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
- **The local media server must already be running** (`./scripts/run_now.sh`)
  before invoking the suite. This suite only health-checks
  `http://127.0.0.1:$SWARM_STUN_PORT/health` and never starts, stops, or
  restarts it, since the server's lifecycle is GUI-owned and it may already
  be serving real traffic (see `swarm-local-testing`). Starting the GUI
  itself isn't something an unattended script should do on someone's
  machine on its own initiative.

The media server must have been started by the current `run_now.sh`, which
passes the debug-only control-file path into the GUI process. A server process
started before this testing-mode support was built must be restarted once;
the suite still never performs that lifecycle change itself.

## Testing-mode security invariants

- The UI toggle and `00000000` behavior are reachable only in the Android
  debug build. A release media server rejects `begin_testing` outright.
- `00000000` is a visible test indicator, not an automation credential. The
  automation path also requires a random 256-bit token delivered to the TV by
  authorized adb and read by the server from a permission-restricted local
  control file.
- Test trust never enters the TV's Room connection history or the server's
  `local_peer` SQLite table. `AllowedPeers` tracks ephemeral grants separately
  so roster refresh cannot erase them and their cleanup cannot erase durable
  user trust.
- Testing uses a separate AndroidKeyStore identity. Startup and disable both
  delete its private key and any journaled derived catalog cache; the server
  independently expires its grant after 10 minutes even if the TV loses power
  before explicit cleanup.
