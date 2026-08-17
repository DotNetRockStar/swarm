#!/usr/bin/env bash
# Real Fire TV logcat, filtered to the SWARM app — for seeing what it's
# doing (or why it crashed) without rebuilding or reinstalling anything.
# Complements deploy_tv.sh, which already prints the crash excerpt
# automatically when ITS OWN launch fails; reach for this instead when you
# launched the app by hand on the TV (tapped the icon on the home screen)
# and want to see what happened.
#
# Usage:
#   ./tv_logs.sh                  # clears the log, waits for you to act on the TV, live-tails
#   ./tv_logs.sh -d                # dumps whatever's already in the buffer and exits (no waiting)
#   ./tv_logs.sh 192.168.0.148     # connects to this IP first
#   ./tv_logs.sh 192.168.0.148 -d  # both together, in either order
#
# Env vars:
#   SWARM_TV_IP    default target IP if none is passed as an argument
#   ANDROID_HOME   default ~/Library/Android/sdk

set -euo pipefail
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
ADB="$ANDROID_HOME/platform-tools/adb"
PACKAGE="app.swarm.tv"

DUMP=0
TARGET=""
for arg in "$@"; do
    case "$arg" in
        -d|--dump) DUMP=1 ;;
        *) TARGET="$arg" ;;
    esac
done
TARGET="${TARGET:-${SWARM_TV_IP:-}}"

if [ -n "$TARGET" ]; then
    [[ "$TARGET" == *:* ]] || TARGET="$TARGET:5555"
    "$ADB" connect "$TARGET" >/dev/null
    SERIAL="$TARGET"
else
    devices="$("$ADB" devices | awk 'NR>1 && $2=="device" {print $1}')"
    count="$(printf '%s\n' "$devices" | grep -c . || true)"
    if [ "$count" -eq 1 ]; then
        SERIAL="$devices"
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
    [ "$STATE" = "unauthorized" ] && echo "Accept the \"Allow USB debugging?\" prompt on the TV screen, then re-run." >&2
    exit 1
fi

if [ "$DUMP" -eq 1 ]; then
    "$ADB" -s "$SERIAL" logcat -d | grep -i "$PACKAGE" || echo "(nothing matching $PACKAGE in the current buffer — try without -d and reproduce it live)"
else
    echo "==> Cleared logcat on $SERIAL. Go open/use the app on the TV now." >&2
    echo "==> Streaming matching lines below, Ctrl+C to stop." >&2
    "$ADB" -s "$SERIAL" logcat -c
    "$ADB" -s "$SERIAL" logcat | grep -i --line-buffered "$PACKAGE"
fi
