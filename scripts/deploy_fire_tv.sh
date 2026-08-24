#!/usr/bin/env bash
# Rebuilds the Fire TV client, installs it on a real device over adb, and
# verifies it actually launches without crashing — automating the exact
# manual cycle that caught a real multidex ClassNotFoundException on real
# Fire OS hardware (SwarmTvApplication landed in classes11.dex; neither
# the emulator-less build nor any unit test could have caught it). Exits
# non-zero on any failure, so it doubles as a repeatable regression check
# after any change, not just a one-off convenience script.
#
# With no IP given and no device already in `adb devices`, this scans the
# local /24 for Fire TVs (port 5555, manufacturer "Amazon") and prompts for
# which one(s) to deploy to. That scan only finds devices that already have
# ADB-over-network reachable — enable it once per device first: Settings ->
# My Fire TV -> Developer Options -> [ADB debugging / Network debugging] ON
# (some Fire OS versions require one initial `adb tcpip 5555` over USB first).
#
# Usage:
#   ./scripts/deploy_fire_tv.sh                # uses $SWARM_TV_IP, the sole device already in `adb devices`, or a LAN scan + prompt
#   ./scripts/deploy_fire_tv.sh 192.168.0.148   # connects to this IP first (find it: Settings -> My Fire TV -> About -> Network)
#   ./scripts/deploy_fire_tv.sh -f              # also tails logcat after a clean launch, until Ctrl+C (single target only)
#   ./scripts/deploy_fire_tv.sh -c              # uninstall first (wipes the device's saved STUN link/swarms/token) — see below
#
# Env vars:
#   SWARM_TV_IP     default target IP if none is passed as an argument (skips the LAN scan)
#   ANDROID_HOME    default ~/Library/Android/sdk
#   JAVA_HOME       default /opt/homebrew/opt/openjdk@17
#   SWARM_LAN_IP    optional LAN-IP override shared with run_now.sh
#   SWARM_RENDEZVOUS_URL  SWARM service embedded in this debug build; when
#                         unset, this script uses this Mac's LAN IP on :8080

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../clients/tv-android"

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export JAVA_HOME="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
ADB="$ANDROID_HOME/platform-tools/adb"
PACKAGE="app.swarm.tv"
# Fully qualified, not "$PACKAGE/.MainActivity": AGP namespace is
# app.swarm.tv but MainActivity's real Kotlin package is app.swarm.tv.app
# (one segment deeper) — a relative name here resolves against the
# namespace and points at a class that doesn't exist. See
# AndroidManifest.xml's comment on the activity element and the
# swarm-real-device-debugging skill for the full story.
ACTIVITY="$PACKAGE/app.swarm.tv.app.MainActivity"
ADB_PORT=5555

FOLLOW=0
CLEAN=0
TARGET=""
for arg in "$@"; do
    case "$arg" in
        -f|--follow) FOLLOW=1 ;;
        -c|--clean) CLEAN=1 ;;
        *) TARGET="$arg" ;;
    esac
done
TARGET="${TARGET:-${SWARM_TV_IP:-}}"

# A debug deploy normally targets the local service started by run_now.sh.
# Embed its current LAN address so a Mac DHCP change does not leave the TV
# retrying an obsolete saved IP forever. An explicit environment value still
# wins for testing against a remote/public service.
if [ -z "${SWARM_RENDEZVOUS_URL:-}" ]; then
    host_ip="${SWARM_LAN_IP:-}"
    if [ -z "$host_ip" ]; then
        for iface in en0 en1; do
            host_ip="$(ipconfig getifaddr "$iface" 2>/dev/null || true)"
            [ -n "$host_ip" ] && break
        done
    fi
    [ -z "$host_ip" ] || export SWARM_RENDEZVOUS_URL="http://$host_ip:8080"
fi
if [ -n "${SWARM_RENDEZVOUS_URL:-}" ]; then
    echo "==> Debug SWARM service: $SWARM_RENDEZVOUS_URL"
else
    echo "==> No SWARM service URL detected; LAN-only discovery will still work."
fi

# Scans this Mac's /24 for hosts with adb's TCP port open, connects to each
# long enough to read ro.product.manufacturer, and keeps only the ones that
# say "Amazon" (real Fire TV hardware) — disconnecting everything else so
# the scan doesn't leave stray adb connections behind. Prints one
# "name<TAB>ip" pair per line to stdout.
scan_lan_for_fire_tvs() {
    local iface subnet_ip prefix open_ips ip manufacturer name
    subnet_ip=""
    for iface in en0 en1 eth0; do
        subnet_ip="$(ipconfig getifaddr "$iface" 2>/dev/null || true)"
        [ -n "$subnet_ip" ] && break
    done
    if [ -z "$subnet_ip" ]; then
        echo "Could not determine this Mac's LAN IPv4 address to scan." >&2
        return 1
    fi
    prefix="${subnet_ip%.*}"

    echo "==> Scanning $prefix.0/24 for Fire TVs (port $ADB_PORT) ..." >&2
    # xargs exits non-zero here whenever at least one of the 254 probes finds
    # a closed port — true on essentially every real LAN — which under
    # `set -e` would otherwise kill the whole script via this assignment.
    open_ips="$(seq 1 254 | xargs -P 64 -I{} bash -c \
        'nc -z -w1 "$1.$2" '"$ADB_PORT"' 2>/dev/null && echo "$1.$2"' _ "$prefix" {} || true)"

    [ -n "$open_ips" ] || return 0
    while IFS= read -r ip; do
        [ -n "$ip" ] || continue
        "$ADB" connect "$ip:$ADB_PORT" >/dev/null 2>&1 || continue
        if [ "$("$ADB" -s "$ip:$ADB_PORT" get-state 2>/dev/null || true)" != "device" ]; then
            "$ADB" disconnect "$ip:$ADB_PORT" >/dev/null 2>&1 || true
            continue
        fi
        manufacturer="$("$ADB" -s "$ip:$ADB_PORT" shell getprop ro.product.manufacturer 2>/dev/null | tr -d '\r')"
        if [[ "$manufacturer" != *[Aa]mazon* ]]; then
            "$ADB" disconnect "$ip:$ADB_PORT" >/dev/null 2>&1 || true
            continue
        fi
        name="$("$ADB" -s "$ip:$ADB_PORT" shell settings get global device_name 2>/dev/null | tr -d '\r')"
        if [ -z "$name" ] || [ "$name" = "null" ]; then
            name="$("$ADB" -s "$ip:$ADB_PORT" shell getprop ro.product.model 2>/dev/null | tr -d '\r')"
        fi
        printf '%s\t%s\n' "${name:-Fire TV}" "$ip"
        "$ADB" disconnect "$ip:$ADB_PORT" >/dev/null 2>&1 || true
    done <<< "$open_ips"
}

SERIALS=()
if [ -n "$TARGET" ]; then
    [[ "$TARGET" == *:* ]] || TARGET="$TARGET:$ADB_PORT"
    echo "==> Connecting to $TARGET ..."
    "$ADB" connect "$TARGET"
    SERIALS=("$TARGET")
else
    devices="$("$ADB" devices | awk 'NR>1 && $2=="device" {print $1}')"
    count="$(printf '%s\n' "$devices" | grep -c . || true)"
    if [ "$count" -eq 1 ]; then
        SERIALS=("$devices")
        echo "==> Using already-connected device ${SERIALS[0]}"
    else
        names=()
        ips=()
        while IFS=$'\t' read -r name ip; do
            [ -n "$ip" ] || continue
            names+=("$name")
            ips+=("$ip")
        done < <(scan_lan_for_fire_tvs)

        if [ "${#ips[@]}" -eq 0 ]; then
            echo "No device IP given, no \$SWARM_TV_IP set, \`adb devices\` doesn't show exactly one connected device, and the LAN scan found no Amazon Fire TVs." >&2
            echo "Usage: $0 <fire-tv-ip>   (find it: Settings -> My Fire TV -> About -> Network)" >&2
            "$ADB" devices >&2
            exit 1
        elif [ "${#ips[@]}" -eq 1 ]; then
            echo "==> Found one Fire TV on the LAN: ${names[0]} | ${ips[0]}"
            SERIALS=("${ips[0]}:$ADB_PORT")
        else
            echo "Found ${#ips[@]} Fire TVs on the LAN:"
            for i in "${!ips[@]}"; do
                printf '  %d) %s | %s\n' "$((i + 1))" "${names[$i]}" "${ips[$i]}"
            done
            read -rp "Deploy to which one? [1-${#ips[@]}, or 'a' for all]: " choice
            if [[ "$choice" =~ ^[Aa](ll)?$ ]]; then
                for ip in "${ips[@]}"; do
                    SERIALS+=("$ip:$ADB_PORT")
                done
            elif [[ "$choice" =~ ^[0-9]+$ ]] && [ "$choice" -ge 1 ] && [ "$choice" -le "${#ips[@]}" ]; then
                SERIALS=("${ips[$((choice - 1))]}:$ADB_PORT")
            else
                echo "Invalid choice: $choice" >&2
                exit 1
            fi
        fi
        for serial in "${SERIALS[@]}"; do
            "$ADB" connect "$serial" >/dev/null
        done
    fi
fi

# bash 3.2 (macOS's default /bin/bash) has no negative array indices, so the
# last serial is spelled out via its count instead of SERIALS[-1].
LAST_SERIAL="${SERIALS[$((${#SERIALS[@]} - 1))]}"
if [ "${#SERIALS[@]}" -gt 1 ] && [ "$FOLLOW" -eq 1 ]; then
    echo "==> -f/--follow only tails one device; it will follow $LAST_SERIAL once every device is deployed."
fi

echo "==> Building debug APK ..."
./gradlew :app:assembleDebug

FAILED=()
for SERIAL in "${SERIALS[@]}"; do
    echo
    echo "==> Deploying to $SERIAL ..."

    STATE="$("$ADB" -s "$SERIAL" get-state 2>/dev/null || echo "unreachable")"
    if [ "$STATE" != "device" ]; then
        echo "Device $SERIAL is '$STATE', not ready." >&2
        if [ "$STATE" = "unauthorized" ]; then
            echo "Accept the \"Allow USB debugging?\" prompt on the TV screen, then re-run." >&2
        fi
        FAILED+=("$SERIAL")
        continue
    fi

    # Real bug, found live: an unconditional uninstall-before-install here wiped
    # the device's saved STUN link/swarm membership/access token (Room DB +
    # EncryptedSharedPreferences, both in the app's private data dir, both gone
    # on uninstall) on *every single redeploy* — reported as "the TV doesn't
    # save the STUN server/device name/SWARM info across resets/redeploy", which
    # was this script doing exactly that on every run, not a real persistence
    # bug in the app itself (AndroidConnectionStore/AndroidTokenStore round-trip
    # correctly otherwise). `gradlew installDebug` already does a plain
    # replace-in-place install (`adb install -r` semantics) on its own — no
    # uninstall needed for the normal case. -c/--clean opts back into the old
    # uninstall-first behavior for when that's actually wanted (recovering from
    # a genuinely corrupted install, or deliberately testing first-run
    # onboarding from a clean slate).
    if [ "$CLEAN" -eq 1 ]; then
        echo "==> --clean: uninstalling any previous install of $PACKAGE on $SERIAL (this wipes its saved STUN link/swarms/token) ..."
        "$ADB" -s "$SERIAL" uninstall "$PACKAGE" || true
    fi

    echo "==> Installing on $SERIAL (in place; add -c/--clean for a fresh uninstall first) ..."
    ANDROID_SERIAL="$SERIAL" ./gradlew :app:installDebug

    echo "==> Force-stopping any previous run and clearing logcat ..."
    "$ADB" -s "$SERIAL" shell am force-stop "$PACKAGE"
    "$ADB" -s "$SERIAL" logcat -c

    echo "==> Launching $ACTIVITY ..."
    "$ADB" -s "$SERIAL" shell am start -n "$ACTIVITY"

    echo "==> Waiting to confirm it stays up (polling for 16s — a real crash on real hardware took >4s to surface once, so a single short sleep isn't trustworthy) ..."
    CRASH=""
    PID=""
    for _ in $(seq 1 8); do
        sleep 2
        CRASH="$("$ADB" -s "$SERIAL" logcat -d | grep "FATAL EXCEPTION" || true)"
        PID="$("$ADB" -s "$SERIAL" shell pidof "$PACKAGE" || true)"
        [ -n "$CRASH" ] && break
    done

    if [ -n "$CRASH" ] || [ -z "$PID" ]; then
        echo
        echo "FAILED: $PACKAGE did not stay running on $SERIAL." >&2
        echo "--- relevant logcat ---" >&2
        "$ADB" -s "$SERIAL" logcat -d | grep -A 25 "FATAL EXCEPTION" >&2 || "$ADB" -s "$SERIAL" logcat -d | tail -40 >&2
        FAILED+=("$SERIAL")
        continue
    fi

    echo
    echo "OK: $PACKAGE is running on $SERIAL (pid $PID)."

    if [ "$FOLLOW" -eq 1 ] && [ "$SERIAL" = "$LAST_SERIAL" ]; then
        echo "==> Tailing logcat on $SERIAL (Ctrl+C to stop) ..."
        "$ADB" -s "$SERIAL" logcat --pid="$PID"
    fi
done

if [ "${#FAILED[@]}" -gt 0 ]; then
    echo
    echo "FAILED on: ${FAILED[*]}" >&2
    exit 1
fi
