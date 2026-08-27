# Closed-loop Fire TV testing

Two suites test the real desktop media server against real Amazon Fire TV
hardware on the same LAN. Both require real hardware and can't run on
GitHub-hosted CI — see [Why local-only](#why-local-only-hardware). Both are
change-controlled: **read the `swarm-e2e-suite-lockdown` skill before editing
either one's test logic.** Neither suite is touched by editing the other.

## TL;DR — run everything

```bash
./scripts/run_now.sh                        # 1. start the local media server (if not already running)
./scripts/tv_e2e_suite.sh                   # 2. fast smoke test — no UI navigation, full fan-out, ~1 min/device
./scripts/tv_uat_suite.sh                   # 3. full UAT suite — real UI navigation, ~16 scenarios, several minutes
```

Run them **one after the other, never at the same time** — both suites point
the TV at one shared testing-mode control file, and it's the exact path the
already-running server was configured with at startup (`run_now.sh` sets
`SWARM_TV_E2E_CONTROL_FILE` once, and the server only ever reads that one
path for the rest of its life). Each suite writes a fresh token there for its
own run and deletes it on exit; if two runs overlap, whichever finishes first
deletes the file the other is still using. Chained with `&&` above, that's
handled for you.

No flags needed for the common case: both suites run against your preferred
device (see below) if one is configured, otherwise they fan out across every
Fire TV they find on the LAN. Both need the server already running — neither
one starts it.

Fast local iteration on one scenario while developing:

```bash
./scripts/tv_uat_suite.sh --test BrowseCatalogUatTest
```

### What to expect

Both suites print progress as they go (device discovery, install, each test
case as it runs), then a Markdown results table, then exit. What a finished
run looks like:

- **Console:** a `PASS: N   FAIL: N   SKIP: N` summary line, followed by a
  `| Device | Serial | Test | Result | Evidence |` table — one row per test
  case per device (`tv_uat_suite.sh`'s rows are `<TestClass>#<method>`).
  `SKIP` means a prior test case on that device failed and cascaded (e.g. the
  app never launched, so nothing downstream could run).
- **Exit code:** `0` only if `FAIL` is `0` across every device tested; `2` if
  a precondition failed before any device was even reached (server not
  running, no server data dir found); otherwise non-zero. Safe to chain in CI
  or a script (`suite.sh && next_step` only proceeds on a real all-clear) and
  safe to check with `echo $?` locally.
- **Local report file:** `.run/tv-e2e-reports/<timestamp>/report.md` and
  `.run/tv-uat-reports/<timestamp>/report.md` respectively (gitignored).
  `tv_uat_suite.sh` also leaves one evidence folder per FAIL alongside its
  report — see [Failure evidence](#failure-evidence-tv_uat_suitesh) below;
  nothing extra is written for a clean PASS run beyond the report and
  per-device logcat.
- **GitHub issue:** filed automatically (`gh issue create`, label `Testing`)
  **only when at least one test failed** — a clean PASS run never files
  anything, it just prints "no failures — nothing to file." `--no-issue`
  suppresses filing even on a FAIL, if you want the local report only.
- **No Fire TV found:** both suites treat this as a *finding*, not a hard
  failure — they'll say so in the report/console (`FAIL_COUNT` stays `0` if
  nothing else failed) rather than erroring out, since running on a network
  with no TV present is a legitimate state, not a bug.

## The two suites

| | `tv_e2e_suite.sh` | `tv_uat_suite.sh` |
|---|---|---|
| What it proves | The app launches, pairs, and completes one real catalog round-trip | Sixteen real UI scenarios: browse, detail, like/watchlist/report-a-problem, playback, music, filters |
| How it asserts | Logcat/adb evidence only — **sends no D-pad input** | Real Compose UI navigation (`androidx.compose.ui.test` + `androidx.test.uiautomator`) |
| SQLite validation | None | Real server (`library.sqlite`) and TV-side (`swarm.db`, `SharedPreferences`) state, cross-checked |
| Device targeting | Preferred device by default (see below); `--all` for full fan-out | Preferred device by default (see below); `--all` for full fan-out |
| On failure | Per-device logcat dump | Full UI-to-server evidence bundle (see below) |
| Runtime per device | ~1 minute | Several minutes (16 scenarios, one plays real video/audio for 30s each) |
| Skill | `swarm-closed-loop-tv-testing` | `swarm-tv-uat-suite` |

Use `tv_e2e_suite.sh` as the fast "did I break the build" check after any
change touching the client, server, or LAN pairing path. Use
`tv_uat_suite.sh` when a change could affect actual screen content, like/
watchlist/report-a-problem behavior, playback, or the filter bar — or run
its `--test` form for just the one scenario your change touches.

## Prerequisites

- The local media server running (`./scripts/run_now.sh`) — both suites only
  health-check it, never start/stop it (GUI-owned lifecycle).
- ADB debugging enabled on each TV, with the one-time on-device "Allow" trust
  prompt already accepted (`deploy_fire_tv.sh` will prompt you through this
  the first time it hits an unauthorized device). This can't be automated —
  it's an Android security control, not a workflow gap.
- `ANDROID_HOME` / `JAVA_HOME` set, or the defaults
  (`~/Library/Android/sdk`, `/opt/homebrew/opt/openjdk@17`) already correct.
- For `tv_uat_suite.sh`'s server-side SQLite checks: the server's real data
  dir must be reachable (default macOS Tauri path, override with
  `SWARM_SERVER_DATA_DIR` if yours differs).

## Preferring one device for local runs

Both suites share one config file, `scripts/tv_test_device.local.json`
(gitignored — not committed) — set it once and both default to that device:

```json
{ "preferred_device_name": "Michael's 4th TV" }
```

Copy `scripts/tv_test_device.local.json.example` to get started. The name is
matched against the TV's own `settings get global device_name`. Local runs
should set this once and get fast, single-device iteration by default; use
`--all` on either suite to force full fan-out (e.g. for a pre-release
comprehensive pass) regardless of the preference file. If the preferred
device isn't found on the LAN, both suites log that and fall back to full
fan-out rather than silently doing nothing.

Override for a single run without touching the config file, on either suite:

```bash
./scripts/tv_e2e_suite.sh --device 192.168.0.148
./scripts/tv_e2e_suite.sh --device "Living Room TV"
./scripts/tv_uat_suite.sh --device 192.168.0.148
./scripts/tv_uat_suite.sh --device "Living Room TV"
```

Device-targeting precedence is the same on both: explicit `--device` (or, on
`tv_e2e_suite.sh`, a bare positional IP/name — kept for backward
compatibility) > preferred device from the config file > every
already-adb-connected Amazon device > full LAN scan fan-out.

## Scenario catalog (`tv_uat_suite.sh`)

| Class | Covers |
|---|---|
| `BrowseCatalogUatTest` | Browse auto-opens; movies/shows/music load with box art; Continue Watching capped at 6; filter bar (media type, Liked-only, genre) |
| `MovieDetailLikeUatTest` | Movie detail screen fields; Like round-trips through the "Liked only" filter |
| `MovieWatchlistUatTest` | Watchlist add/remove round-trip, toasts, Watchlist row |
| `MovieProblemReportUatTest` | Report-a-problem → dismiss; and the server-resolve round-trip |
| `MoviePlaybackPauseUatTest` | Movie playback, FF/RW, pause overlay fields, resume, no Next Episode button |
| `ShowPlaybackPauseUatTest` | Episode playback, pause overlay, Next Episode button |
| `ShowSeasonsEpisodesWatchlistUatTest` | Season/episode structure, show watchlist round-trip |
| `MusicPlaybackUatTest` | Artist → album → track browsing, per-track Like, shuffle/skip, mini-player |

Full detail on each scenario, known deviations from earlier scenario
wording (e.g. "reviews" doesn't exist as a field — the pause overlay shows
an aggregate rating instead), and the server-resolve mechanism live in the
`swarm-tv-uat-suite` skill.

## Failure evidence (`tv_uat_suite.sh`)

Every failed scenario writes a self-contained folder under
`.run/tv-uat-reports/<run>/<device>/<test>/` (gitignored) with everything
needed to debug without touching real hardware again:

- **TV side:** screenshot, UIAutomator window-hierarchy XML, Compose
  semantics tree, logcat since test start, a `swarm.db` dump, and the
  liked/watchlist/watch-state `SharedPreferences` files.
- **Server side:** the relevant `library.sqlite` rows (library entries,
  likes, problem reports, notifications), `server-state.sqlite`, and a tail
  of `logs/server.log` (server request tracing — method/path/status/latency/
  device name — added specifically to make this bundle useful).

`tv_e2e_suite.sh` keeps a simpler per-device logcat dump under
`.run/tv-e2e-reports/<run>/`.

## Invocation reference

```bash
# tv_e2e_suite.sh
./scripts/tv_e2e_suite.sh                    # preferred device if configured, else full fan-out
./scripts/tv_e2e_suite.sh --all              # force full fan-out
./scripts/tv_e2e_suite.sh --device 192.168.0.148   # one device, by IP or device_name
./scripts/tv_e2e_suite.sh 192.168.0.148      # same, positional form (backward compatible)
./scripts/tv_e2e_suite.sh --no-issue         # skip filing a GitHub issue
./scripts/tv_e2e_suite.sh --skip-install     # smoke-test an already-installed build

# tv_uat_suite.sh
./scripts/tv_uat_suite.sh                        # preferred device if configured, else full fan-out
./scripts/tv_uat_suite.sh --all                  # force full fan-out
./scripts/tv_uat_suite.sh --device 192.168.0.148 # one device, by IP or device_name
./scripts/tv_uat_suite.sh --test BrowseCatalogUatTest            # one scenario class
./scripts/tv_uat_suite.sh --test MusicPlaybackUatTest#testLike   # one scenario method
./scripts/tv_uat_suite.sh --no-issue             # skip filing a GitHub issue
./scripts/tv_uat_suite.sh --skip-install         # smoke-test an already-installed build
```

Both suites are plain deterministic scripts — a human, a CI runner (once a
self-hosted LAN runner exists; see below), and an AI agent all invoke the
exact same command with the exact same result. Nothing in either suite calls
an LLM at run time.

## Why local-only hardware

Both suites need: real Fire TV hardware physically present, on the same LAN
as the machine running the suite, already `adb`-trust-authorized (a one-time
human tap on the device), plus the local media server already running.
GitHub-hosted runners can provide none of that, so there is no
`.github/workflows` entry for either suite today — they're designed to be
CI-friendly (clean exit codes, machine-parseable results) for whenever a
self-hosted LAN runner exists, without one being wired up now.

## Changing either suite

Both suites' test logic, thresholds, fan-out behavior, and (for the UAT
suite) evidence-bundle contents are frozen by explicit user policy — **read
`swarm-e2e-suite-lockdown` before editing either.** A FAIL is a real bug
report: fix the product code, don't loosen the test. Genuine infrastructure
bugs in the scripts themselves (shell quoting, adb timing races, an
instrumentation-output parsing edge case) are fair game to fix without
asking — see the skill for exactly where that line is.
