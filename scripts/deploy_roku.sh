#!/usr/bin/env bash
# Builds the Roku client (clients/tv-roku), packages it, and sideloads it
# onto a real device over Roku's developer install API — the Roku
# analogue of deploy_fire_tv.sh. Exits non-zero on any failure.
#
# First time only, per device: enable Developer Mode --
#   Home x3, Up x2, Right, Left, Right, Left, Right
# on the physical remote, then set a developer password when prompted.
# That IP + password pair is this script's target.
#
# With no explicit target, this scans the local /24 for Roku devices
# (ECP on port 8060, which answers /query/device-info even before
# Developer Mode's install API is used), lists every match, and prompts
# for which one to deploy to. Developer Mode itself must already be on
# for /plugin_install to work — the ECP scan alone can't tell you that.
#
# Usage:
#   ./scripts/deploy_roku.sh                  # scans the LAN, lists Rokus, and prompts for the target
#   ./scripts/deploy_roku.sh 192.168.0.150     # connects to this IP directly (Settings -> Network -> About)
#   ./scripts/deploy_roku.sh -l                # also launches the channel via ECP after a clean install
#
# Env vars:
#   SWARM_ROKU_IP        default target IP if none is passed as an argument (skips the LAN scan)
#   SWARM_ROKU_PASSWORD  Developer Mode password (prompted for if unset)
#   SWARM_ROKU_NAME      preferred device name shown in the LAN choices

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../clients/tv-roku"

BSC="./node_modules/.bin/bsc"
# Deliberately staged *inside* the project directory, not $TMPDIR:
# bsconfig.json's relative paths (rootDir, outFile, files globs) resolve
# relative to wherever the config file itself lives, not the invoking
# shell's cwd -- confirmed live (an /tmp-staged copy silently resolved
# rootDir to a nonexistent /tmp/src and produced an empty package with no
# manifest). Cleaned up via the trap below regardless of exit path.
STAGED_CONFIG="./.deploy-bsconfig.json"
trap 'rm -f "$STAGED_CONFIG"' EXIT
OUT_ZIP="./out/swarm-tv-roku.zip"
LAUNCH_AFTER=false

while getopts "l" opt; do
    case "$opt" in
    l) LAUNCH_AFTER=true ;;
    *) ;;
    esac
done
shift $((OPTIND - 1))

TARGET_IP="${1:-${SWARM_ROKU_IP:-}}"

if [ ! -x "$BSC" ]; then
    echo "brighterscript compiler not found — run 'npm install' in clients/tv-roku first." >&2
    exit 1
fi

# ---- discovery ----

scan_for_rokus() {
    echo "Scanning the local network for Roku devices (ECP :8060)..." >&2
    local subnet
    subnet="$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null || true)"
    if [ -z "$subnet" ]; then
        echo "Could not determine this machine's LAN IP — pass a target IP directly." >&2
        return 1
    fi
    local prefix="${subnet%.*}"
    local found=()
    for i in $(seq 1 254); do
        (
            resp="$(curl -s -m 1 "http://$prefix.$i:8060/query/device-info" 2>/dev/null || true)"
            if [ -n "$resp" ] && echo "$resp" | grep -q "<device-info>"; then
                name="$(echo "$resp" | sed -n 's:.*<user-device-name>\(.*\)</user-device-name>.*:\1:p')"
                model="$(echo "$resp" | sed -n 's:.*<model-name>\(.*\)</model-name>.*:\1:p')"
                echo "$prefix.$i|$name|$model"
            fi
        ) &
    done
    wait
}

if [ -z "$TARGET_IP" ]; then
    mapfile -t RESULTS < <(scan_for_rokus | sort -t. -k4 -n)
    if [ "${#RESULTS[@]}" -eq 0 ]; then
        echo "No Roku devices found. Pass an IP directly: ./scripts/deploy_roku.sh <ip>" >&2
        exit 1
    fi
    echo "Found ${#RESULTS[@]} Roku device(s):"
    i=1
    for row in "${RESULTS[@]}"; do
        IFS='|' read -r ip name model <<<"$row"
        echo "  [$i] $ip  -  ${name:-unnamed}  (${model:-unknown model})"
        i=$((i + 1))
    done
    read -rp "Select a device [1-${#RESULTS[@]}]: " choice
    IFS='|' read -r TARGET_IP _ _ <<<"${RESULTS[$((choice - 1))]}"
fi

echo "Target: $TARGET_IP"

ROKU_PASSWORD="${SWARM_ROKU_PASSWORD:-}"
if [ -z "$ROKU_PASSWORD" ]; then
    read -rsp "Developer Mode password for $TARGET_IP: " ROKU_PASSWORD
    echo
fi

# ---- build ----

echo "Compiling..."
python3 - "$STAGED_CONFIG" <<'PYEOF'
import json, sys
with open("bsconfig.json") as f:
    config = json.load(f)
config["createPackage"] = True
with open(sys.argv[1], "w") as f:
    json.dump(config, f)
PYEOF

"$BSC" --project "$STAGED_CONFIG"

if [ ! -f "$OUT_ZIP" ]; then
    echo "Build did not produce $OUT_ZIP" >&2
    exit 1
fi

# ---- sideload ----

echo "Installing on $TARGET_IP..."
INSTALL_RESPONSE="$(curl -s -u "rokudev:$ROKU_PASSWORD" \
    -F "mysubmit=Install" \
    -F "archive=@$OUT_ZIP" \
    "http://$TARGET_IP/plugin_install")"

if echo "$INSTALL_RESPONSE" | grep -qi "Identical to previous version\|Install Success\|updated"; then
    echo "Installed."
elif echo "$INSTALL_RESPONSE" | grep -qi "Unauthorized\|401"; then
    echo "Authorization failed — check the Developer Mode password." >&2
    exit 1
elif echo "$INSTALL_RESPONSE" | grep -qi "Compile Error\|Failure"; then
    echo "The device rejected the package:" >&2
    echo "$INSTALL_RESPONSE" | grep -i -A2 "Compile Error\|Failure" >&2 || true
    exit 1
else
    echo "Install request completed (response did not match a known success/failure pattern — verify on-device)." >&2
fi

if [ "$LAUNCH_AFTER" = true ]; then
    echo "Launching..."
    curl -s -u "rokudev:$ROKU_PASSWORD" -d "" "http://$TARGET_IP:8060/launch/dev" >/dev/null
fi

echo "Done."
