#!/usr/bin/env bash
# Closed-loop end-to-end test suite: local media server <-> real Amazon Fire
# TV(s) on the same network. "Closed loop" means every assertion here is
# evidence a real device actually observed, not a mock or an emulator — see
# swarm-closed-loop-tv-testing (skill) for the methodology this implements
# and swarm-e2e-suite-lockdown (skill) for the change policy on this file:
# do not edit test logic, pass/fail thresholds, or the fan-out behavior
# below to make a failing run go green. Fix the underlying product bug, or
# leave the failure in the findings report, unless the user explicitly asks
# for a change to the suite itself in the current conversation.
#
# This suite never starts or stops the media server: it is GUI-owned (see
# swarm-local-testing skill) and may already be serving real traffic. It
# only checks that one is reachable before running.
#
# Usage:
#   ./scripts/tv_e2e_suite.sh                    # LAN-scan; fan out across every discovered Amazon Fire TV
#   ./scripts/tv_e2e_suite.sh 192.168.0.148       # test only these device(s) (repeat the arg for more than one)
#   ./scripts/tv_e2e_suite.sh --no-issue          # write the report locally; skip posting it to GitHub
#   ./scripts/tv_e2e_suite.sh --skip-install      # smoke-test whatever build is already installed; no rebuild/reinstall
#
# Env vars:
#   SWARM_STUN_PORT          local rendezvous HTTP port to health-check (default 8080)
#   ANDROID_HOME             default ~/Library/Android/sdk
#   JAVA_HOME                default /opt/homebrew/opt/openjdk@17
#   SWARM_GITHUB_REPOSITORY  where findings are filed (default DotNetRockStar/swarm)
#   SWARM_E2E_ISSUE_LABEL    label applied to the findings issue (default "Testing")

set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export JAVA_HOME="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
ADB="$ANDROID_HOME/platform-tools/adb"
PACKAGE="app.swarm.tv"
# Fully qualified, not "$PACKAGE/.MainActivity" — see swarm-real-device-debugging.
ACTIVITY="$PACKAGE/app.swarm.tv.app.MainActivity"
ADB_PORT=5555
STUN_PORT="${SWARM_STUN_PORT:-8080}"
GITHUB_REPOSITORY="${SWARM_GITHUB_REPOSITORY:-DotNetRockStar/swarm}"
ISSUE_LABEL="${SWARM_E2E_ISSUE_LABEL:-Testing}"

NO_ISSUE=0
SKIP_INSTALL=0
TARGETS=()
for arg in "$@"; do
    case "$arg" in
        --no-issue) NO_ISSUE=1 ;;
        --skip-install) SKIP_INSTALL=1 ;;
        *) TARGETS+=("$arg") ;;
    esac
done

RUN_STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
REPORT_DIR="$REPO_ROOT/.run/tv-e2e-reports/$RUN_STAMP"
mkdir -p "$REPORT_DIR"
RESULTS_TSV="$REPORT_DIR/results.tsv"
: > "$RESULTS_TSV"

echo "==> Preflight: local media server must already be running (this suite never starts one) ..."
if ! curl -s -o /dev/null -w '' --max-time 3 "http://127.0.0.1:$STUN_PORT/health"; then
    echo "No SWARM rendezvous service answered http://127.0.0.1:$STUN_PORT/health." >&2
    echo "Start it first: ./scripts/run_now.sh" >&2
    exit 2
fi
echo "OK: rendezvous service is up on :$STUN_PORT."

# Intentionally duplicated from deploy_fire_tv.sh rather than shared, so this
# frozen suite cannot silently change behavior if that script is edited later.
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
        # </dev/null on every `adb shell` call in this function: without it,
        # `adb shell` reads from the loop's own input stream (the `<<<`
        # below), so the first device's shell call silently swallows every
        # remaining IP and only device #1 is ever found — confirmed live
        # against 3 real Fire TVs (same root cause fixed in deploy_fire_tv.sh).
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

SERIALS=()
NAMES=()
if [ "${#TARGETS[@]}" -gt 0 ]; then
    for t in "${TARGETS[@]}"; do
        [[ "$t" == *:* ]] || t="$t:$ADB_PORT"
        "$ADB" connect "$t" >/dev/null 2>&1 || true
        SERIALS+=("$t")
        NAMES+=("$t")
    done
else
    echo "==> No explicit target given; checking already-connected devices ..."
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        serial="$(awk '{print $1}' <<< "$line")"
        # </dev/null: same stdin-consumption hazard as scan_lan_for_fire_tvs
        # above — this loop also reads from its own input stream (the
        # process substitution below).
        manufacturer="$("$ADB" -s "$serial" shell getprop ro.product.manufacturer </dev/null 2>/dev/null | tr -d '\r')"
        [[ "$manufacturer" == *[Aa]mazon* ]] || continue
        name="$("$ADB" -s "$serial" shell settings get global device_name </dev/null 2>/dev/null | tr -d '\r')"
        [ -n "$name" ] && [ "$name" != "null" ] || name="$("$ADB" -s "$serial" shell getprop ro.product.model </dev/null 2>/dev/null | tr -d '\r')"
        SERIALS+=("$serial")
        NAMES+=("${name:-Fire TV}")
    done < <("$ADB" devices | awk 'NR>1 && $2=="device" {print $0}')

    if [ "${#SERIALS[@]}" -eq 0 ]; then
        echo "==> No already-connected Amazon Fire TV; scanning the LAN ..."
        while IFS=$'\t' read -r name ip; do
            [ -n "$ip" ] || continue
            NAMES+=("$name")
            SERIALS+=("$ip:$ADB_PORT")
        done < <(scan_lan_for_fire_tvs)
    fi
fi

if [ "${#SERIALS[@]}" -eq 0 ]; then
    echo "No Amazon Fire TVs found on this LAN (nothing already connected via adb, and the LAN scan found none)." | tee "$REPORT_DIR/report.md"
    echo "" >> "$REPORT_DIR/report.md"
    echo "This is reported as a finding rather than a hard failure: the suite requires real hardware on the same network as the machine running it." >> "$REPORT_DIR/report.md"
else
    echo "==> Fanning out across ${#SERIALS[@]} device(s): ${NAMES[*]}"

    if [ "$SKIP_INSTALL" -eq 0 ]; then
        echo "==> Building debug APK ..."
        (cd clients/tv-android && ./gradlew :app:assembleDebug)
    fi

    record() {
        # $1 serial  $2 name  $3 test  $4 PASS|FAIL|SKIP  $5 evidence (single line)
        printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" >> "$RESULTS_TSV"
    }

    for i in "${!SERIALS[@]}"; do
        SERIAL="${SERIALS[$i]}"
        NAME="${NAMES[$i]}"
        SAFE_NAME="$(echo "$SERIAL" | tr -c 'A-Za-z0-9._-' '_')"
        DEVICE_LOG="$REPORT_DIR/$SAFE_NAME.logcat.txt"

        echo
        echo "==> [$NAME @ $SERIAL] connecting ..."
        "$ADB" connect "$SERIAL" >/dev/null 2>&1 || true
        STATE="$("$ADB" -s "$SERIAL" get-state 2>/dev/null || echo "unreachable")"
        if [ "$STATE" != "device" ]; then
            record "$SERIAL" "$NAME" "install_and_launch" "FAIL" "adb state=$STATE (not ready/authorized)"
            record "$SERIAL" "$NAME" "lan_closed_loop_catalog" "SKIP" "skipped: device was not reachable over adb"
            continue
        fi

        if [ "$SKIP_INSTALL" -eq 0 ]; then
            echo "==> [$NAME] installing (in place, no wipe) ..."
            if ! (cd clients/tv-android && ANDROID_SERIAL="$SERIAL" ./gradlew :app:installDebug); then
                record "$SERIAL" "$NAME" "install_and_launch" "FAIL" "gradlew :app:installDebug failed"
                record "$SERIAL" "$NAME" "lan_closed_loop_catalog" "SKIP" "skipped: install failed"
                continue
            fi
        fi

        echo "==> [$NAME] force-stopping, clearing logcat, launching ..."
        "$ADB" -s "$SERIAL" shell am force-stop "$PACKAGE"
        "$ADB" -s "$SERIAL" logcat -c
        "$ADB" -s "$SERIAL" shell am start -n "$ACTIVITY" >/dev/null

        echo "==> [$NAME] polling up to 16s to confirm it stays up ..."
        CRASH=""
        PID=""
        for _ in $(seq 1 8); do
            sleep 2
            CRASH="$("$ADB" -s "$SERIAL" logcat -d | grep "FATAL EXCEPTION" || true)"
            PID="$("$ADB" -s "$SERIAL" shell pidof "$PACKAGE" 2>/dev/null || true)"
            [ -n "$CRASH" ] && break
        done
        "$ADB" -s "$SERIAL" logcat -d > "$DEVICE_LOG" || true

        if [ -n "$CRASH" ] || [ -z "$PID" ]; then
            record "$SERIAL" "$NAME" "install_and_launch" "FAIL" "$(echo "$CRASH" | head -1 | tr -d '\t' || echo 'process did not stay running')"
            record "$SERIAL" "$NAME" "lan_closed_loop_catalog" "SKIP" "skipped: app did not launch cleanly"
            continue
        fi
        record "$SERIAL" "$NAME" "install_and_launch" "PASS" "pid $PID stable for 16s"
        echo "OK: [$NAME] $PACKAGE is running (pid $PID)."

        echo "==> [$NAME] waiting up to 20s for the automatic LAN reconnect + catalog refresh (no D-pad input sent) ..."
        REFRESH_LINE=""
        FAIL_LINE=""
        for _ in $(seq 1 10); do
            sleep 2
            DUMP="$("$ADB" -s "$SERIAL" logcat -d)"
            REFRESH_LINE="$(grep "browseCatalog() refresh done: entries=" <<< "$DUMP" | tail -1 || true)"
            [ -n "$REFRESH_LINE" ] && break
            FAIL_LINE="$(grep -E "leaving it unreachable for this attempt|could not probe the saved LAN server during startup" <<< "$DUMP" | tail -1 || true)"
        done
        "$ADB" -s "$SERIAL" logcat -d > "$DEVICE_LOG" || true

        if [ -n "$REFRESH_LINE" ]; then
            ENTRIES="$(sed -E 's/.*entries=([0-9]+).*/\1/' <<< "$REFRESH_LINE")"
            UNREACHABLE="$(sed -E 's/.*unreachable=([0-9]+).*/\1/' <<< "$REFRESH_LINE")"
            if [ "${UNREACHABLE:-0}" = "0" ]; then
                record "$SERIAL" "$NAME" "lan_closed_loop_catalog" "PASS" "server responded: entries=$ENTRIES unreachable=$UNREACHABLE"
            else
                record "$SERIAL" "$NAME" "lan_closed_loop_catalog" "FAIL" "server reachable but reported unreachable peers: entries=$ENTRIES unreachable=$UNREACHABLE"
            fi
        elif [ -n "$FAIL_LINE" ]; then
            record "$SERIAL" "$NAME" "lan_closed_loop_catalog" "FAIL" "$(echo "$FAIL_LINE" | tr -d '\t')"
        else
            record "$SERIAL" "$NAME" "lan_closed_loop_catalog" "SKIP" "no saved LAN connection observed on this device within 20s (pair it first via the media server's Swarm page; this suite does not drive first-time D-pad pairing, see swarm-real-device-debugging)"
        fi
    done
fi

# --- Compile the Markdown findings report ---
COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
PASS_COUNT="$(awk -F'\t' '$4=="PASS"' "$RESULTS_TSV" 2>/dev/null | wc -l | tr -d ' ')"
FAIL_COUNT="$(awk -F'\t' '$4=="FAIL"' "$RESULTS_TSV" 2>/dev/null | wc -l | tr -d ' ')"
SKIP_COUNT="$(awk -F'\t' '$4=="SKIP"' "$RESULTS_TSV" 2>/dev/null | wc -l | tr -d ' ')"

{
    echo "# TV closed-loop E2E suite — $RUN_STAMP"
    echo
    echo "Local media server + \`${#SERIALS[@]}\` real Amazon Fire TV(s) on the LAN, exercised by \`scripts/tv_e2e_suite.sh\` against commit \`$COMMIT\`."
    echo
    echo "**PASS: $PASS_COUNT   FAIL: $FAIL_COUNT   SKIP: $SKIP_COUNT**"
    echo
    if [ -s "$RESULTS_TSV" ]; then
        echo "| Device | Serial | Test | Result | Evidence |"
        echo "|---|---|---|---|---|"
        while IFS=$'\t' read -r serial name test result evidence; do
            evidence_escaped="$(echo "$evidence" | sed 's/|/\\|/g')"
            echo "| $name | $serial | $test | $result | $evidence_escaped |"
        done < "$RESULTS_TSV"
    fi
    echo
    echo "Full per-device logcat captures were kept locally under \`.run/tv-e2e-reports/$RUN_STAMP/\` (not committed)."
    echo
    echo "---"
    echo "_This suite's test logic and pass/fail thresholds are frozen by policy — see the \`swarm-e2e-suite-lockdown\` skill. An AI agent picking up a follow-up on this issue should fix the underlying product bug behind any FAIL above, not edit \`scripts/tv_e2e_suite.sh\` to make it pass._"
} > "$REPORT_DIR/report.md"

echo
cat "$REPORT_DIR/report.md"
echo
echo "==> Report written to $REPORT_DIR/report.md"

if [ "$NO_ISSUE" -eq 0 ] && command -v gh >/dev/null 2>&1; then
    TITLE="TV closed-loop E2E suite: $PASS_COUNT passed, $FAIL_COUNT failed, $SKIP_COUNT skipped ($RUN_STAMP)"
    if ISSUE_URL="$(gh issue create --repo "$GITHUB_REPOSITORY" --title "$TITLE" --body-file "$REPORT_DIR/report.md" --label "$ISSUE_LABEL" 2>&1)"; then
        echo "==> Findings posted: $ISSUE_URL"
    else
        echo "Could not post findings to GitHub (report is still saved locally): $ISSUE_URL" >&2
    fi
fi

[ "$FAIL_COUNT" -eq 0 ]
