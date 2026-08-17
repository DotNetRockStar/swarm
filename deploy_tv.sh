#!/usr/bin/env bash
# Rebuilds the Fire TV client, installs it on a real device over adb, and
# verifies it actually launches without crashing — automating the exact
# manual cycle that caught a real multidex ClassNotFoundException on real
# Fire OS hardware (SwarmTvApplication landed in classes11.dex; neither
# the emulator-less build nor any unit test could have caught it). Exits
# non-zero on any failure, so it doubles as a repeatable regression check
# after any change, not just a one-off convenience script.
#
# Usage:
#   ./deploy_tv.sh                  # uses $SWARM_TV_IP, or the sole device already in `adb devices`
#   ./deploy_tv.sh 192.168.0.148    # connects to this IP first (find it: Settings -> My Fire TV -> About -> Network)
#   ./deploy_tv.sh -f                # also tails logcat after a clean launch, until Ctrl+C
#
# Env vars:
#   SWARM_TV_IP     default target IP if none is passed as an argument
#   ANDROID_HOME    default ~/Library/Android/sdk
#   JAVA_HOME       default /opt/homebrew/opt/openjdk@17

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/clients/tv-android"

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

FOLLOW=0
TARGET=""
for arg in "$@"; do
    case "$arg" in
        -f|--follow) FOLLOW=1 ;;
        *) TARGET="$arg" ;;
    esac
done
TARGET="${TARGET:-${SWARM_TV_IP:-}}"

if [ -n "$TARGET" ]; then
    [[ "$TARGET" == *:* ]] || TARGET="$TARGET:5555"
    echo "==> Connecting to $TARGET ..."
    "$ADB" connect "$TARGET"
    SERIAL="$TARGET"
else
    devices="$("$ADB" devices | awk 'NR>1 && $2=="device" {print $1}')"
    count="$(printf '%s\n' "$devices" | grep -c . || true)"
    if [ "$count" -eq 1 ]; then
        SERIAL="$devices"
        echo "==> Using already-connected device $SERIAL"
    else
        echo "No device IP given, no \$SWARM_TV_IP set, and \`adb devices\` doesn't show exactly one connected device." >&2
        echo "Usage: $0 <fire-tv-ip>   (find it: Settings -> My Fire TV -> About -> Network)" >&2
        "$ADB" devices >&2
        exit 1
    fi
fi

STATE="$("$ADB" -s "$SERIAL" get-state 2>/dev/null || echo "unreachable")"
if [ "$STATE" != "device" ]; then
    echo "Device $SERIAL is '$STATE', not ready." >&2
    if [ "$STATE" = "unauthorized" ]; then
        echo "Accept the \"Allow USB debugging?\" prompt on the TV screen, then re-run." >&2
    fi
    exit 1
fi

echo "==> Building debug APK ..."
./gradlew :app:assembleDebug

# Full uninstall before installing, not just an in-place replace — cheap
# insurance against any stale per-package state on the device rather than
# a proven requirement. (A real ClassNotFoundException crash was chased
# down to this device's dex handling before the actual cause turned out
# to be a manifest/package-name mismatch unrelated to install semantics —
# see swarm-real-device-debugging skill. Left in as a low-cost precaution,
# not because reinstalling without it was ever confirmed to fail.)
echo "==> Uninstalling any previous install of $PACKAGE on $SERIAL ..."
"$ADB" -s "$SERIAL" uninstall "$PACKAGE" || true

echo "==> Installing on $SERIAL ..."
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
    echo "FAILED: $PACKAGE did not stay running." >&2
    echo "--- relevant logcat ---" >&2
    "$ADB" -s "$SERIAL" logcat -d | grep -A 25 "FATAL EXCEPTION" >&2 || "$ADB" -s "$SERIAL" logcat -d | tail -40 >&2
    exit 1
fi

echo
echo "OK: $PACKAGE is running on $SERIAL (pid $PID)."

if [ "$FOLLOW" -eq 1 ]; then
    echo "==> Tailing logcat (Ctrl+C to stop) ..."
    "$ADB" -s "$SERIAL" logcat --pid="$PID"
fi
