---
name: swarm-tv-uat-suite
description: Use when running, extending, or explaining scripts/tv_uat_suite.sh — the UI-driving UAT/integration suite that navigates the real Fire TV app (browse, detail, like/watchlist/report-a-problem, playback, music) against the real local media server, cross-checking real SQLite state, and produces a full UI-to-server evidence bundle on any failure. This is the second, larger closed-loop suite alongside scripts/tv_e2e_suite.sh (see swarm-closed-loop-tv-testing) — read swarm-e2e-suite-lockdown before editing test logic in either.
---

# TV UAT suite: real UI navigation against the real media server

`./scripts/tv_uat_suite.sh` drives the actual Fire TV app UI (D-pad
navigation, real button presses) against a real, already-running local media
server, asserting on real Compose UI state, real persisted TV-side state
(Room `swarm.db` + the liked/watchlist/watch-state `SharedPreferences`
stores, read in-process), and real server-side SQLite state
(`library.sqlite`'s `library_entries`, `entry_likes`, `client_errors`,
`server_notifications` tables). Its change policy is the same explicit,
standing user rule as `scripts/tv_e2e_suite.sh` — **read
`swarm-e2e-suite-lockdown` before editing anything here.**

## How this differs from `scripts/tv_e2e_suite.sh`

The original suite (`swarm-closed-loop-tv-testing`) deliberately sends no
D-pad input at all — it proves the app launches and completes one real
catalog round-trip using only logcat/adb evidence. This suite exists because
the user explicitly asked for scenario coverage the original suite was never
meant to provide: opening a movie, liking it, adding to a watchlist,
reporting a problem and watching it resolve, playing and pausing video,
browsing a show's seasons/episodes, playing music, and exercising the filter
bar. That coverage requires real UI navigation, so this suite uses
`androidx.compose.ui.test` + `androidx.test.uiautomator` instrumented tests
instead of pure log-evidence — element lookup goes through stable Compose
`testTag`s (see `UatTestTags.kt` in the TV client's main UI source set), not
brittle text matching, so scenario tests survive copy changes.

Both suites coexist permanently and are frozen independently. Neither
suite's own file is touched by changes to the other.

## Scenario catalog

Each class lives under
`clients/tv-android/app/src/androidTest/kotlin/app/swarm/tv/app/uat/`; each
`@Test` method's KDoc references the scenario number from the conversation
that specified it.

| Class | Scenarios | Covers |
|---|---|---|
| `BrowseCatalogUatTest` | 1, 2, 3, 4, 5, 6, 16 | Browse auto-opens without clicking in; movies/shows/music load; box art loads for a movie, a show, a music artist; Continue Watching never shows more than 6; the filter bar's media-type (All/Movies/Shows/Music), Liked-only, and genre controls exist and work |
| `MovieDetailLikeUatTest` | 7, 8 | A random movie's detail screen has box art, year, genres, cast, description, Play/Like/Watchlist/Report-a-problem buttons; Like round-trips through the "Liked only" filter |
| `MovieWatchlistUatTest` | 9 | Watchlist add/remove round-trips, including the toast copy and the Watchlist genre row |
| `MovieProblemReportUatTest` | 10, 11 | Report-a-problem → toast → notification inbox → dismiss; and the server-resolve round-trip (see below) |
| `MoviePlaybackPauseUatTest` | 12 | Movie playback: play, fast-forward, rewind, pause overlay fields (asserts the aggregate rating shown, not free-text "reviews" — see Known deviations below), resume, settings, back-navigation stack |
| `ShowPlaybackPauseUatTest` | 13 | Same pause-overlay checks for an episode, plus the Next Episode button (present and functional — unlike the movie case) |
| `ShowSeasonsEpisodesWatchlistUatTest` | 14 | Season list with box art/episode counts, watchlist round-trip on a show, episode grid sequencing and non-duplication |
| `MusicPlaybackUatTest` | 15 | Artist → album → track browsing, per-track like round-trip through the filter, shuffle/skip/pause/resume, mini-player reopen/close |

**Suggested additional coverage**, added under the same explicit-request
rule as everything else here once the user asks for a specific one: repeated
Back-button navigation-stack integrity from a deep screen, kid-mode content
gating, combined filters, notification badge-count accuracy.

## Known deviations from the literal scenario wording (confirmed with the user)

- **"Reviews" doesn't exist as a field.** The pause overlay shows a
  community rating + vote count (`★ X.X/10 (N votes)`); scenario 12 asserts
  that instead of free-text reviews.
- **A movie's pause overlay correctly has no Next Episode button** (the app
  only shows it for `kind == EPISODE`). Scenario 12 asserts its absence;
  scenario 13 (a show/episode) asserts its presence and that it works.
- **"Favorites" and "Liked only" are the same feature** — there's no
  separate Favorites tab. Scenario 16 asserts the "Liked only" toggle.
- **Watchlist and continue-watching have no SQLite backing anywhere**
  (neither `library.sqlite` nor the TV's Room `swarm.db` — both live only in
  `SharedPreferences`). Those scenarios validate against the app's real
  persisted `SharedPreferences` store, read in-process from inside the
  instrumented test, rather than SQL.

## The server-resolve round-trip (scenario 11)

The dashboard's "Resolve" button is a Tauri desktop-GUI command with no
existing HTTP route, so an unattended run can't click it. `apps/server`
exposes a debug-build-only `POST /errors/{id}/resolve` endpoint (mirroring
the existing dismiss endpoint's gating) specifically for this suite. The
flow: the instrumented test submits a real problem report through the UI,
confirms the "Problem Report Sent." toast, then emits a logcat checkpoint
(`UAT_AWAITING_SERVER_RESOLVE`) and waits. The orchestration script watches
for that checkpoint while the instrumentation run is still in progress,
queries `client_errors` for the most recently received still-unresolved row
(safe because scenario classes run sequentially per device — never two
report-a-problem tests concurrently against the same server), and calls the
debug resolve endpoint with `{"comments":"test"}` — then the test continues,
asserting the real notification-inbox UI shows the resolution (the app's own
`syncResolutionNotifications` polling, not a mock). This is the one scenario
where the orchestration script takes an action mid-run rather than only
observing. Scenario 10 (dismiss) uses the same mechanism to get a
notification to dismiss in the first place — the inbox only ever surfaces
*resolved* reports, so there is no unresolved-notification state to test
independently of resolution.

## Device targeting: preferred device vs. fan-out

Precedence: explicit `--device <ip-or-name>` > preferred device from
`scripts/tv_test_device.local.json` (gitignored; example at
`scripts/tv_test_device.local.json.example`) > every already-adb-connected Amazon
device > full LAN scan fan-out.

```json
{ "preferred_device_name": "Michael's 4th TV" }
```

The name is matched against the same `settings get global device_name` read
the original suite already uses. **Local runs should prefer one TV** (fast
iteration, one real device is enough evidence for most changes) — set this
file once. `--all` forces full fan-out across every discovered Fire TV
regardless of the preference file, for comprehensive runs. If the preferred
device isn't found on the LAN, the script logs that and falls back to full
fan-out rather than silently doing nothing.

`scripts/tv_e2e_suite.sh` reads this exact same config file and follows the
identical precedence (see its own "Fan-out discovery" section in
`swarm-closed-loop-tv-testing`) — set the preferred device once and both
suites default to it.

## Invocation

```
./scripts/tv_uat_suite.sh                        # preferred device if configured, else full fan-out
./scripts/tv_uat_suite.sh --all                   # force full fan-out
./scripts/tv_uat_suite.sh --device 192.168.0.148  # one specific device, by IP or device_name
./scripts/tv_uat_suite.sh --test BrowseCatalogUatTest             # one scenario class
./scripts/tv_uat_suite.sh --test MusicPlaybackUatTest#testLike    # one scenario method
./scripts/tv_uat_suite.sh --no-issue              # skip filing to GitHub
./scripts/tv_uat_suite.sh --skip-install          # smoke-test an already-installed build
```

Nothing here calls an LLM at run time — a human, a CI runner (once a
self-hosted LAN runner exists; there is no GitHub-hosted path, same
hardware/LAN/adb-trust reasons as the original suite), and an AI agent all
invoke the exact same deterministic script. `--test` is the cheap path for
iterating on one scenario without re-running all sixteen.

## Failure evidence: the full UI-to-server dump

Every FAIL writes a self-contained folder under
`.run/tv-uat-reports/<run>/<device>/<TestClass>#<method>/` (gitignored, not
attached to the filed issue) containing:

- **TV side:** a screenshot, the UIAutomator window-hierarchy XML, the
  Compose semantics tree, logcat since test start (all captured in-process
  by `UatFailureCaptureRule` and pulled off-device), a `swarm.db` dump, and
  the liked/watchlist/watch-state `SharedPreferences` XML files.
- **Server side:** the relevant rows from `library.sqlite`
  (`library_entries`/`entry_likes`/`client_errors`/`server_notifications`)
  and `server-state.sqlite`, and a tail of `logs/server.log` — the request
  tracing added alongside this suite (method/path/status/latency/device
  name) is what makes it possible to see what the server actually did in
  response to the TV action that failed.

This exists specifically so a follow-up debugging session doesn't need to
reproduce the failure on real hardware just to see what happened — the
bundle should be enough on its own, per the user's explicit requirement.

## Requirements this suite shares with the original

- ADB debugging authorized once per TV (human taps "Allow" on-device) — same
  Android security control, not automatable.
- The local media server must already be running (`./scripts/run_now.sh`);
  this suite never starts, stops, or restarts it.
- First-time D-pad pairing is never driven by this suite either — it uses
  the same debug-only testing-mode token mechanism as the original suite.
