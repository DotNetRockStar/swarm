#!/usr/bin/env bash
# See scripts/tests/TV_TESTING.md for the TL;DR on running this suite alongside
# scripts/tests/tv_e2e_suite.sh (they must not run at the same time — both point
# at one shared testing-mode control file the already-running server was
# configured with at startup), and what a completed run looks like.
#
# UAT / integration suite: drives the real Fire TV UI (browse, detail, like,
# watchlist, report-a-problem, playback, music) against the real local media
# server, cross-checking real SQLite state on both sides where such state
# exists. This is a SEPARATE, newer suite from scripts/tests/tv_e2e_suite.sh (which
# stays log-evidence-only, no UI navigation, and is untouched by this file).
# Both suites' test logic, thresholds, and fan-out/evidence behavior are
# frozen by explicit user policy — see swarm-e2e-suite-lockdown (skill). Do
# not edit scenario assertions or evidence collection here to make a failing
# run go green: fix the underlying product bug, or leave the failure in the
# findings report, unless the user explicitly asks for a change to this
# suite in the current conversation. See swarm-tv-uat-suite (skill) for the
# full scenario catalog and evidence model this implements.
#
# This suite never starts or stops the media server (GUI-owned lifecycle,
# see swarm-local-testing) and never drives first-time D-pad pairing — it
# uses the same debug-only testing-mode mechanism tv_e2e_suite.sh uses.
#
# Usage:
#   ./scripts/tests/tv_uat_suite.sh                        # preferred device (scripts/tests/tv_test_device.local.json) if configured, else full LAN fan-out
#   ./scripts/tests/tv_uat_suite.sh --all                   # force full fan-out across every discovered Fire TV, ignoring the preferred-device config
#   ./scripts/tests/tv_uat_suite.sh --device 192.168.0.148  # test only this device (IP or adb device_name)
#   ./scripts/tests/tv_uat_suite.sh --test BrowseCatalogUatTest            # run one scenario class
#   ./scripts/tests/tv_uat_suite.sh --test MusicPlaybackUatTest#testLike   # run one scenario method
#   ./scripts/tests/tv_uat_suite.sh --github-issue           # opt in to filing a GitHub issue when failures occur
#   ./scripts/tests/tv_uat_suite.sh --no-issue               # explicit local-only mode (the default; retained for compatibility)
#   ./scripts/tests/tv_uat_suite.sh --skip-install          # smoke-test whatever build is already installed; no rebuild/reinstall
#
# Env vars:
#   SWARM_STUN_PORT          local rendezvous HTTP port to health-check (default 8080)
#   SWARM_HTTP_MEDIA_PORT    local media server HTTP port, for the debug-only resolve hook (default 8546)
#   SWARM_SERVER_DATA_DIR    media server app-data dir holding library.sqlite/server-state.sqlite/logs
#                            (default: macOS Tauri app_data_dir for app.swarm.server)
#   ANDROID_HOME             default ~/Library/Android/sdk
#   JAVA_HOME                default /opt/homebrew/opt/openjdk@17
#   SWARM_TV_TEST_PACKAGE    instrumentation test package (default app.swarm.tv.test)
#   SWARM_TV_TEST_RUNNER     instrumentation runner class (default androidx.test.runner.AndroidJUnitRunner)
#   SWARM_GITHUB_REPOSITORY  where findings are filed (default DotNetRockStar/swarm)
#   SWARM_E2E_ISSUE_LABEL    label applied to the findings issue (default "Testing")
#   SWARM_RUN_DIR            shared local run directory (default .run)

set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export JAVA_HOME="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
ADB="$ANDROID_HOME/platform-tools/adb"
PACKAGE="app.swarm.tv"
ACTIVITY="$PACKAGE/app.swarm.tv.app.MainActivity"
TEST_PACKAGE="${SWARM_TV_TEST_PACKAGE:-app.swarm.tv.test}"
TEST_RUNNER="${SWARM_TV_TEST_RUNNER:-androidx.test.runner.AndroidJUnitRunner}"
UAT_TEST_NAMESPACE="app.swarm.tv.app.uat"
ADB_PORT=5555
STUN_PORT="${SWARM_STUN_PORT:-8080}"
HTTP_MEDIA_PORT="${SWARM_HTTP_MEDIA_PORT:-8546}"
GITHUB_REPOSITORY="${SWARM_GITHUB_REPOSITORY:-DotNetRockStar/swarm}"
ISSUE_LABEL="${SWARM_E2E_ISSUE_LABEL:-Testing}"
RUN_DIR="${SWARM_RUN_DIR:-.run}"
TV_E2E_CONTROL_FILE="$RUN_DIR/tv-e2e-control.json"
PREFERRED_DEVICE_FILE="scripts/tests/tv_test_device.local.json"

# Same directory the real macOS Tauri app_data_dir() resolves to for
# app.swarm.server (see apps/server/src/gui.rs) — override if your server
# runs with a different data dir.
SERVER_DATA_DIR="${SWARM_SERVER_DATA_DIR:-$HOME/Library/Application Support/app.swarm.server}"
SERVER_LIBRARY_DB="$SERVER_DATA_DIR/library.sqlite"
SERVER_STATE_DB="$SERVER_DATA_DIR/server-state.sqlite"
# tracing-appender's daily rolling writer names files server.log.<date>
# (e.g. server.log.2026-08-28), never a bare "server.log" — resolve the
# newest matching file at evidence-collection time rather than a fixed name,
# so an evidence bundle is never silently missing the server-side log.
SERVER_LOG_DIR="$SERVER_DATA_DIR/logs"

# The full scenario catalog run by default (no --test). One class per
# scenario group — see swarm-tv-uat-suite (skill) for what each covers.
ALL_TEST_CLASSES=(
    BrowseCatalogUatTest
    MovieDetailLikeUatTest
    MovieWatchlistUatTest
    MovieProblemReportUatTest
    MoviePlaybackPauseUatTest
    ShowPlaybackPauseUatTest
    ShowSeasonsEpisodesWatchlistUatTest
    MusicPlaybackUatTest
    NavigationSearchPersistenceUatTest
    ContinuePlaybackLifecycleUatTest
    KidModeUatTest
    EndOfMediaUatTest
)

ALL_MODE=0
POST_GITHUB_ISSUE=0
GITHUB_ISSUE_FLAG=0
NO_ISSUE_FLAG=0
SKIP_INSTALL=0
DEVICE_ARG=""
TEST_ARG=""
while [ $# -gt 0 ]; do
    case "$1" in
        --all) ALL_MODE=1 ;;
        --github-issue) POST_GITHUB_ISSUE=1; GITHUB_ISSUE_FLAG=1 ;;
        --no-issue) POST_GITHUB_ISSUE=0; NO_ISSUE_FLAG=1 ;;
        --skip-install) SKIP_INSTALL=1 ;;
        --device) shift; DEVICE_ARG="${1:-}" ;;
        --test) shift; TEST_ARG="${1:-}" ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
    shift || true
done

if [ "$GITHUB_ISSUE_FLAG" -eq 1 ] && [ "$NO_ISSUE_FLAG" -eq 1 ]; then
    echo "--github-issue and --no-issue cannot be used together." >&2
    exit 2
fi

if [ -n "$TEST_ARG" ]; then
    # Accept a bare class name or "Class#method"; qualify with the UAT
    # package if the caller didn't already pass a fully-qualified name.
    if [[ "$TEST_ARG" == *.* ]]; then
        RUN_CLASS_SPEC="$TEST_ARG"
    else
        RUN_CLASS_SPEC="$UAT_TEST_NAMESPACE.$TEST_ARG"
    fi
else
    joined=""
    for c in "${ALL_TEST_CLASSES[@]}"; do
        joined="${joined:+$joined,}$UAT_TEST_NAMESPACE.$c"
    done
    RUN_CLASS_SPEC="$joined"
fi

mkdir -p "$RUN_DIR"
TESTING_TOKEN="$(openssl rand -hex 32)"
TESTING_CONTROL_TMP="${TV_E2E_CONTROL_FILE}.tmp.$$"
refresh_testing_control() {
    local expires_at
    expires_at="$(( $(date +%s) + 600 ))"
    (umask 077 && printf '{"token":"%s","expires_at_unix_seconds":%s}\n' \
        "$TESTING_TOKEN" "$expires_at" > "$TESTING_CONTROL_TMP")
    chmod 600 "$TESTING_CONTROL_TMP"
    mv "$TESTING_CONTROL_TMP" "$TV_E2E_CONTROL_FILE"
}
refresh_testing_control
cleanup_testing_control() { rm -f "$TV_E2E_CONTROL_FILE" "$TESTING_CONTROL_TMP"; }
trap cleanup_testing_control EXIT INT TERM

RUN_STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
REPORT_DIR="$REPO_ROOT/.run/tv-uat-reports/$RUN_STAMP"
mkdir -p "$REPORT_DIR"
RESULTS_TSV="$REPORT_DIR/results.tsv"
: > "$RESULTS_TSV"

echo "==> Preflight: local media server must already be running (this suite never starts one) ..."
if ! curl -s -o /dev/null -w '' --max-time 3 "http://127.0.0.1:$STUN_PORT/health"; then
    echo "No SWARM rendezvous service answered http://127.0.0.1:$STUN_PORT/health." >&2
    echo "Start it first: ./scripts/run_now.sh" >&2
    exit 2
fi
if [ ! -f "$SERVER_LIBRARY_DB" ]; then
    echo "No server library database found at $SERVER_LIBRARY_DB — this suite validates real SQLite state, so it needs the real server data dir. Set SWARM_SERVER_DATA_DIR if yours differs." >&2
    exit 2
fi
echo "OK: rendezvous service is up on :$STUN_PORT; server data dir found at $SERVER_DATA_DIR."

# Intentionally duplicated from tv_e2e_suite.sh / deploy_fire_tv.sh rather
# than shared, so neither frozen suite's discovery can change out from
# under the other if one of them is edited later — see swarm-e2e-suite-lockdown.
scan_lan_for_fire_tvs() {
    local iface subnet_ip prefix live_ips open_ips ip manufacturer name
    subnet_ip=""
    for iface in en0 en1 eth0; do
        subnet_ip="$(ipconfig getifaddr "$iface" 2>/dev/null || true)"
        [ -n "$subnet_ip" ] && break
    done
    if [ -z "$subnet_ip" ]; then
        echo "Could not determine this Mac's LAN IPv4 address to scan." >&2
        return 0
    fi
    prefix="${subnet_ip%.*}"

    echo "==> Pinging $prefix.0/24 to find live hosts ..." >&2
    live_ips="$(seq 1 254 | xargs -P 64 -I{} bash -c \
        'ping -c1 -t1 -q "$1.$2" >/dev/null 2>&1 && echo "$1.$2"' _ "$prefix" {} || true)"
    [ -n "$live_ips" ] || return 0

    echo "==> Checking live hosts for adb (port $ADB_PORT) ..." >&2
    open_ips="$(printf '%s\n' "$live_ips" | xargs -P 32 -I{} bash -c \
        'nc -z -w1 "$1" '"$ADB_PORT"' 2>/dev/null && echo "$1"' _ {} || true)"
    [ -n "$open_ips" ] || return 0

    while IFS= read -r ip; do
        [ -n "$ip" ] || continue
        "$ADB" connect "$ip:$ADB_PORT" >/dev/null 2>&1 || continue
        if [ "$("$ADB" -s "$ip:$ADB_PORT" get-state 2>/dev/null || true)" != "device" ]; then
            "$ADB" disconnect "$ip:$ADB_PORT" >/dev/null 2>&1 || true
            continue
        fi
        # </dev/null on every `adb shell` call: without it, `adb shell` reads
        # from this loop's own input stream and only device #1 is ever found
        # — see swarm-closed-loop-tv-testing for the confirmed-live repro.
        manufacturer="$("$ADB" -s "$ip:$ADB_PORT" shell getprop ro.product.manufacturer </dev/null 2>/dev/null | tr -d '\r')"
        if [[ "$manufacturer" != *[Aa]mazon* ]]; then
            "$ADB" disconnect "$ip:$ADB_PORT" >/dev/null 2>&1 || true
            continue
        fi
        name="$("$ADB" -s "$ip:$ADB_PORT" shell settings get global device_name </dev/null 2>/dev/null | tr -d '\r')"
        if [ -z "$name" ] || [ "$name" = "null" ]; then
            name="$("$ADB" -s "$ip:$ADB_PORT" shell getprop ro.product.model </dev/null 2>/dev/null | tr -d '\r')"
        fi
        printf '%s\t%s\n' "${name:-Fire TV}" "$ip"
    done <<< "$open_ips"
}

PREFERRED_NAME=""
if [ -f "$PREFERRED_DEVICE_FILE" ]; then
    PREFERRED_NAME="$(grep -o '"preferred_device_name"[[:space:]]*:[[:space:]]*"[^"]*"' "$PREFERRED_DEVICE_FILE" \
        | sed -E 's/.*:[[:space:]]*"([^"]*)"/\1/' | head -1)"
fi

SERIALS=()
NAMES=()
if [ -n "$DEVICE_ARG" ]; then
    if [[ "$DEVICE_ARG" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+(:[0-9]+)?$ ]]; then
        t="$DEVICE_ARG"
        [[ "$t" == *:* ]] || t="$t:$ADB_PORT"
        "$ADB" connect "$t" >/dev/null 2>&1 || true
        SERIALS=("$t"); NAMES=("$t")
    else
        echo "==> Looking for a device named \"$DEVICE_ARG\" ..."
        while IFS=$'\t' read -r name ip; do
            [ -n "$ip" ] || continue
            if [ "$name" = "$DEVICE_ARG" ]; then
                SERIALS+=("$ip:$ADB_PORT"); NAMES+=("$name"); break
            fi
        done < <(scan_lan_for_fire_tvs)
        if [ "${#SERIALS[@]}" -eq 0 ]; then
            echo "No Fire TV named \"$DEVICE_ARG\" found on the LAN." >&2
            exit 2
        fi
    fi
elif [ "$ALL_MODE" -eq 0 ] && [ -n "$PREFERRED_NAME" ]; then
    echo "==> Preferred device configured (\"$PREFERRED_NAME\", from $PREFERRED_DEVICE_FILE); looking for it on the LAN ..."
    while IFS=$'\t' read -r name ip; do
        [ -n "$ip" ] || continue
        if [ "$name" = "$PREFERRED_NAME" ]; then
            SERIALS+=("$ip:$ADB_PORT"); NAMES+=("$name"); break
        fi
    done < <(scan_lan_for_fire_tvs)
    if [ "${#SERIALS[@]}" -eq 0 ]; then
        echo "Preferred device \"$PREFERRED_NAME\" not found on the LAN right now; falling back to full fan-out across every discovered Fire TV." >&2
    fi
fi

if [ "${#SERIALS[@]}" -eq 0 ]; then
    if [ "$ALL_MODE" -eq 1 ]; then
        echo "==> --all: fanning out across every discovered Amazon Fire TV ..."
    else
        echo "==> No device/preferred-device match; checking already-connected devices, then scanning the LAN ..."
    fi
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        serial="$(awk '{print $1}' <<< "$line")"
        manufacturer="$("$ADB" -s "$serial" shell getprop ro.product.manufacturer </dev/null 2>/dev/null | tr -d '\r')"
        [[ "$manufacturer" == *[Aa]mazon* ]] || continue
        name="$("$ADB" -s "$serial" shell settings get global device_name </dev/null 2>/dev/null | tr -d '\r')"
        [ -n "$name" ] && [ "$name" != "null" ] || name="$("$ADB" -s "$serial" shell getprop ro.product.model </dev/null 2>/dev/null | tr -d '\r')"
        SERIALS+=("$serial"); NAMES+=("${name:-Fire TV}")
    done < <("$ADB" devices | awk 'NR>1 && $2=="device" {print $0}')

    if [ "${#SERIALS[@]}" -eq 0 ]; then
        while IFS=$'\t' read -r name ip; do
            [ -n "$ip" ] || continue
            NAMES+=("$name"); SERIALS+=("$ip:$ADB_PORT")
        done < <(scan_lan_for_fire_tvs)
    fi
fi

if [ "${#SERIALS[@]}" -eq 0 ]; then
    echo "No Amazon Fire TVs found on this LAN." | tee "$REPORT_DIR/report.md"
    echo "" >> "$REPORT_DIR/report.md"
    echo "This is reported as a finding rather than a hard failure: the suite requires real hardware on the same network as the machine running it." >> "$REPORT_DIR/report.md"
else
    echo "==> Running against ${#SERIALS[@]} device(s): ${NAMES[*]}"
    echo "==> Scenario classes: $RUN_CLASS_SPEC"

    if [ "$SKIP_INSTALL" -eq 0 ]; then
        echo "==> Building debug app + test APKs ..."
        (cd clients/tv-android && ./gradlew :app:assembleDebug :app:assembleDebugAndroidTest)
    fi

    record() {
        # $1 serial  $2 name  $3 test  $4 PASS|FAIL|SKIP  $5 evidence (single line)  $6 evidence dir (optional, relative)
        printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" "${6:-}" >> "$RESULTS_TSV"
    }

    # --- Server-side evidence helpers -----------------------------------
    server_query() {
        # $1 db path  $2 SQL
        sqlite3 -readonly -separator ' | ' "$1" "$2" 2>/dev/null || true
    }

    dump_server_evidence() {
        # $1 destination dir
        local dest="$1"
        mkdir -p "$dest"
        {
            echo "== library_entries (recent) =="
            server_query "$SERVER_LIBRARY_DB" "SELECT entry_key, kind, title FROM library_entries ORDER BY rowid DESC LIMIT 20;"
            echo
            echo "== entry_likes (recent) =="
            server_query "$SERVER_LIBRARY_DB" "SELECT entry_key, device_id, device_name, liked_at_ms FROM entry_likes ORDER BY liked_at_ms DESC LIMIT 20;"
            echo
            echo "== client_errors (recent) =="
            server_query "$SERVER_LIBRARY_DB" "SELECT id, device_name, asset_title, message, resolution_comments, resolved_at_ms, dismissed_at_ms FROM client_errors ORDER BY received_at_ms DESC LIMIT 20;"
            echo
            echo "== server_notifications (recent) =="
            server_query "$SERVER_LIBRARY_DB" "SELECT level, title, message, created_at_ms FROM server_notifications ORDER BY created_at_ms DESC LIMIT 20;"
        } > "$dest/server_db_dump.txt"
        if [ -f "$SERVER_STATE_DB" ]; then
            server_query "$SERVER_STATE_DB" "SELECT * FROM http_media_device;" > "$dest/server_state_http_media_device.txt" 2>/dev/null || true
        fi
        local newest_log
        newest_log="$(ls -t "$SERVER_LOG_DIR"/server.log.* 2>/dev/null | head -1)"
        if [ -n "$newest_log" ] && [ -f "$newest_log" ]; then
            tail -n 500 "$newest_log" > "$dest/server_log_tail.txt" 2>/dev/null || true
        else
            echo "No server log file found under $SERVER_LOG_DIR/server.log.*" > "$dest/server_log_tail.txt"
        fi
        [ -f "$SERVER_DATA_DIR/settings.json" ] && cp "$SERVER_DATA_DIR/settings.json" "$dest/server_settings.json" 2>/dev/null || true
    }

    dump_tv_evidence() {
        # $1 serial  $2 destination dir  $3 on-device failure dir for this test (already known from UAT_TEST_FAILED line)
        local serial="$1" dest="$2" on_device_dir="$3"
        mkdir -p "$dest"
        "$ADB" -s "$serial" logcat -d > "$dest/logcat.txt" 2>/dev/null || true
        if [ -n "$on_device_dir" ]; then
            "$ADB" -s "$serial" pull "$on_device_dir" "$dest/on_device_failure_capture" >/dev/null 2>&1 || true
        fi
        "$ADB" -s "$serial" shell run-as "$PACKAGE" cat databases/swarm.db </dev/null > "$dest/tv_swarm.db" 2>/dev/null || true
        for pref in liked_entries watchlist watch_state; do
            "$ADB" -s "$serial" shell run-as "$PACKAGE" cat "shared_prefs/${pref}.xml" </dev/null > "$dest/tv_prefs_${pref}.xml" 2>/dev/null || true
        done
    }

    for i in "${!SERIALS[@]}"; do
        SERIAL="${SERIALS[$i]}"
        NAME="${NAMES[$i]}"
        SAFE_NAME="$(echo "$SERIAL" | tr -c 'A-Za-z0-9._-' '_')"
        DEVICE_DIR="$REPORT_DIR/$SAFE_NAME"
        mkdir -p "$DEVICE_DIR"

        echo
        echo "==> [$NAME @ $SERIAL] connecting ..."
        "$ADB" connect "$SERIAL" >/dev/null 2>&1 || true
        STATE="$("$ADB" -s "$SERIAL" get-state 2>/dev/null || echo "unreachable")"
        if [ "$STATE" != "device" ]; then
            record "$SERIAL" "$NAME" "uat_suite" "FAIL" "adb state=$STATE (not ready/authorized)" ""
            continue
        fi

        WAKEFULNESS="$("$ADB" -s "$SERIAL" shell dumpsys power </dev/null 2>/dev/null | grep -m1 'mWakefulness=' | sed -E 's/.*mWakefulness=([A-Za-z]+).*/\1/')"
        if [ "$WAKEFULNESS" != "Awake" ]; then
            "$ADB" -s "$SERIAL" shell input keyevent KEYCODE_WAKEUP </dev/null >/dev/null 2>&1 || true
            sleep 1
        fi

        if [ "$SKIP_INSTALL" -eq 0 ]; then
            echo "==> [$NAME] installing app + test APKs (in place, no wipe) ..."
            if ! (cd clients/tv-android && ANDROID_SERIAL="$SERIAL" ./gradlew :app:installDebug :app:installDebugAndroidTest); then
                record "$SERIAL" "$NAME" "uat_suite" "FAIL" "gradlew install failed" ""
                continue
            fi
        fi

        "$ADB" -s "$SERIAL" logcat -c
        RAW_LOG="$DEVICE_DIR/instrumentation_raw.txt"
        : > "$RAW_LOG"

        # Installation/discovery and a multi-scenario run can exceed the
        # control secret's 10-minute authorization window. Refresh that
        # authorization while the suite owns the control file; each TV-side
        # testing activation still has its independent hard 10-minute TTL,
        # and the trap removes authorization as soon as this run exits.
        refresh_testing_control
        NEXT_TESTING_CONTROL_REFRESH="$(( $(date +%s) + 300 ))"
        echo "==> [$NAME] running: $RUN_CLASS_SPEC ..."
        "$ADB" -s "$SERIAL" shell am instrument -w -r \
            -e class "$RUN_CLASS_SPEC" \
            -e ENABLE_TESTING_MODE true \
            -e TESTING_TOKEN "$TESTING_TOKEN" \
            "$TEST_PACKAGE/$TEST_RUNNER" </dev/null > "$RAW_LOG" 2>&1 &
        INSTRUMENT_PID=$!

        # While instrumentation runs, watch logcat for the one scenario that
        # needs a host-side action mid-run: a problem report awaiting
        # server-side resolution (test 11). Everything else is a no-op here.
        # Correlates by "most recently received, still-unresolved" report
        # rather than by title (the instrumented test has no reliable way to
        # read the movie's title text back out of the UI — no testTag covers
        # it) — safe because this suite runs its scenario classes
        # sequentially per device, never two report-a-problem tests
        # concurrently against the same server.
        RESOLVED_IDS=""
        while kill -0 "$INSTRUMENT_PID" 2>/dev/null; do
            sleep 2
            if [ "$(date +%s)" -ge "$NEXT_TESTING_CONTROL_REFRESH" ]; then
                refresh_testing_control
                NEXT_TESTING_CONTROL_REFRESH="$(( $(date +%s) + 300 ))"
            fi
            CHECKPOINT="$("$ADB" -s "$SERIAL" logcat -d 2>/dev/null | grep -c 'UAT_AWAITING_SERVER_RESOLVE' || true)"
            if [ "${CHECKPOINT:-0}" -gt 0 ]; then
                ERROR_ID="$(server_query "$SERVER_LIBRARY_DB" \
                    "SELECT id FROM client_errors WHERE resolved_at_ms IS NULL ORDER BY received_at_ms DESC LIMIT 1;" | tr -d ' ')"
                if [ -n "$ERROR_ID" ] && [[ "$RESOLVED_IDS" != *"|$ERROR_ID|"* ]]; then
                    echo "==> [$NAME] resolving problem report id=$ERROR_ID via debug resolve endpoint ..."
                    curl -s -X POST "http://127.0.0.1:$HTTP_MEDIA_PORT/errors/$ERROR_ID/resolve" \
                        -H 'Content-Type: application/json' -d '{"comments":"test"}' >/dev/null || true
                    RESOLVED_IDS="$RESOLVED_IDS|$ERROR_ID|"
                fi
            fi
        done
        wait "$INSTRUMENT_PID" 2>/dev/null || true
        "$ADB" -s "$SERIAL" logcat -d > "$DEVICE_DIR/logcat_full.txt" || true

        # --- Parse am instrument -r raw output into per-test PASS/FAIL ---
        awk -v serial="$SERIAL" -v name="$NAME" -v results="$RESULTS_TSV" '
            /^INSTRUMENTATION_STATUS: class=/ { cls=$0; sub(/^INSTRUMENTATION_STATUS: class=/, "", cls) }
            /^INSTRUMENTATION_STATUS: test=/ { tst=$0; sub(/^INSTRUMENTATION_STATUS: test=/, "", tst) }
            /^INSTRUMENTATION_STATUS: stack=/ { instack=1; stack=$0; sub(/^INSTRUMENTATION_STATUS: stack=/, "", stack); next }
            instack==1 {
                if ($0 ~ /^INSTRUMENTATION_STATUS_CODE:/) { instack=0 } else { stack = stack " | " $0; next }
            }
            /^INSTRUMENTATION_STATUS_CODE: -2/ || /^INSTRUMENTATION_STATUS_CODE: -1/ {
                key = cls "#" tst
                if (!(key in seen_fail)) {
                    seen_fail[key]=1
                    ev = stack; gsub(/\t/, " ", ev); if (length(ev) > 300) ev = substr(ev, 1, 300) "..."
                    printf "%s\t%s\t%s\t%s\t%s\t%s\n", serial, name, key, "FAIL", ev, "" >> results
                }
                stack=""
            }
            /^INSTRUMENTATION_STATUS_CODE: 0/ {
                key = cls "#" tst
                if (key != "#" && !(key in seen_start)) { seen_start[key]=1 }
            }
            END {
                for (k in seen_start) if (!(k in seen_fail)) printf "%s\t%s\t%s\t%s\t%s\t%s\n", serial, name, k, "PASS", "completed with no failure/error status", "" >> results
            }
        ' "$RAW_LOG"

        # --- Evidence bundle for every FAIL this device just produced ---
        while IFS=$'\t' read -r r_serial r_name r_test r_result r_evidence _; do
            [ "$r_serial" = "$SERIAL" ] || continue
            [ "$r_result" = "FAIL" ] || continue
            EVIDENCE_DIR="$REPORT_DIR/$SAFE_NAME/${r_test//[:\/#]/_}"
            ON_DEVICE_FAILURE_DIR="/sdcard/Android/data/$PACKAGE/files/uat-failures/${r_test//#/_}"
            echo "==> [$NAME] collecting failure evidence for $r_test ..."
            dump_tv_evidence "$SERIAL" "$EVIDENCE_DIR" "$ON_DEVICE_FAILURE_DIR"
            dump_server_evidence "$EVIDENCE_DIR"
            {
                echo "# Failure evidence: $r_test on $NAME ($SERIAL)"
                echo
                echo "Evidence: $r_evidence"
                echo
                echo "Files: logcat.txt, on_device_failure_capture/ (screenshot, hierarchy XML, Compose semantics dump),"
                echo "tv_swarm.db, tv_prefs_*.xml, server_db_dump.txt, server_state_http_media_device.txt, server_log_tail.txt, server_settings.json"
            } > "$EVIDENCE_DIR/summary.md"
        done < "$RESULTS_TSV"
    done
fi

# --- Compile the Markdown findings report ---
COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
PASS_COUNT="$(awk -F'\t' '$4=="PASS"' "$RESULTS_TSV" 2>/dev/null | wc -l | tr -d ' ')"
FAIL_COUNT="$(awk -F'\t' '$4=="FAIL"' "$RESULTS_TSV" 2>/dev/null | wc -l | tr -d ' ')"
SKIP_COUNT="$(awk -F'\t' '$4=="SKIP"' "$RESULTS_TSV" 2>/dev/null | wc -l | tr -d ' ')"

{
    echo "# TV UAT suite — $RUN_STAMP"
    echo
    echo "Local media server + \`${#SERIALS[@]}\` real Amazon Fire TV(s) on the LAN, exercised by \`scripts/tests/tv_uat_suite.sh\` against commit \`$COMMIT\`."
    echo
    echo "**PASS: $PASS_COUNT   FAIL: $FAIL_COUNT   SKIP: $SKIP_COUNT**"
    echo
    if [ -s "$RESULTS_TSV" ]; then
        echo "| Device | Serial | Test | Result | Evidence |"
        echo "|---|---|---|---|---|"
        while IFS=$'\t' read -r serial name test result evidence _; do
            evidence_escaped="$(echo "$evidence" | sed 's/|/\\|/g')"
            echo "| $name | $serial | $test | $result | $evidence_escaped |"
        done < "$RESULTS_TSV"
    fi
    echo
    echo "Full per-test evidence bundles (logcat, screenshots, hierarchy dumps, TV + server SQLite state, server log tail) are kept locally under \`.run/tv-uat-reports/$RUN_STAMP/<device>/<test>/\` for every FAIL (not committed)."
    echo
    echo "---"
    echo "_This suite's test logic, thresholds, and evidence collection are frozen by policy — see the \`swarm-e2e-suite-lockdown\` skill. An AI agent picking up a follow-up on this issue should fix the underlying product bug behind any FAIL above (evidence bundle has the full UI-to-server trace), not edit \`scripts/tests/tv_uat_suite.sh\` or the \`uat\` test sources to make it pass._"
} > "$REPORT_DIR/report.md"

echo
cat "$REPORT_DIR/report.md"
echo
echo "==> Report written to $REPORT_DIR/report.md"

if [ "$POST_GITHUB_ISSUE" -eq 1 ]; then
    if [ "$FAIL_COUNT" -eq 0 ]; then
        echo "==> No failures — nothing to file. GitHub issues are only opened when FAIL_COUNT > 0."
    elif ! command -v gh >/dev/null 2>&1; then
        echo "Could not post findings to GitHub because gh is unavailable (report is still saved locally)." >&2
    else
        TITLE="TV UAT suite: $PASS_COUNT passed, $FAIL_COUNT failed, $SKIP_COUNT skipped ($RUN_STAMP)"
        if ISSUE_URL="$(gh issue create --repo "$GITHUB_REPOSITORY" --title "$TITLE" --body-file "$REPORT_DIR/report.md" --label "$ISSUE_LABEL" 2>&1)"; then
            echo "==> Findings posted: $ISSUE_URL"
        else
            echo "Could not post findings to GitHub (report is still saved locally): $ISSUE_URL" >&2
        fi
    fi
else
    echo "==> GitHub issue reporting is disabled; use --github-issue to opt in."
fi

[ "$FAIL_COUNT" -eq 0 ]
