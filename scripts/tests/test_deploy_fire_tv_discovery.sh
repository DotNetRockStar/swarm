#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEPLOY="$REPO_ROOT/scripts/deploy_fire_tv.sh"

bash -n "$DEPLOY"

temporary_dir="$(mktemp -d)"
trap 'rm -rf "$temporary_dir"' EXIT
fake_bin="$temporary_dir/bin"
fake_sdk="$temporary_dir/android-sdk"
mkdir -p "$fake_bin" "$fake_sdk/platform-tools"

apply_executable() {
    chmod +x "$1"
}

cat > "$fake_bin/ipconfig" <<'SCRIPT'
#!/usr/bin/env bash
[ "${1:-}" = "getifaddr" ] && [ "${2:-}" = "en0" ] && echo "192.168.50.10"
SCRIPT
apply_executable "$fake_bin/ipconfig"

cat > "$fake_bin/ping" <<'SCRIPT'
#!/usr/bin/env bash
case "${*: -1}" in
    192.168.50.20|192.168.50.30) exit 0 ;;
    *) exit 1 ;;
esac
SCRIPT
apply_executable "$fake_bin/ping"

cat > "$fake_bin/nc" <<'SCRIPT'
#!/usr/bin/env bash
exit 0
SCRIPT
apply_executable "$fake_bin/nc"

cat > "$fake_bin/sleep" <<'SCRIPT'
#!/usr/bin/env bash
exit 0
SCRIPT
apply_executable "$fake_bin/sleep"

cat > "$fake_sdk/platform-tools/adb" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$FAKE_ADB_LOG"

case "${1:-}" in
    connect) echo "connected to ${2:-}"; exit 0 ;;
    disconnect|kill-server|start-server) exit 0 ;;
    devices)
        printf 'List of devices attached\n192.168.50.99:5555\tdevice\n'
        exit 0
        ;;
esac

if [ "${1:-}" = "-s" ]; then
    serial="$2"
    shift 2
    case "${1:-} ${2:-} ${3:-} ${4:-}" in
        "get-state   ") echo device ;;
        "shell getprop ro.product.manufacturer ") echo Amazon ;;
        "shell settings get global")
            case "$serial" in
                192.168.50.20:5555) echo "Living Room TV" ;;
                192.168.50.30:5555) echo "Bedroom TV" ;;
                *) echo "Other TV" ;;
            esac
            ;;
        "shell pidof app.swarm.tv ") echo 4242 ;;
        *) exit 0 ;;
    esac
fi
SCRIPT
apply_executable "$fake_sdk/platform-tools/adb"

cat > "$temporary_dir/gradlew" <<'SCRIPT'
#!/usr/bin/env bash
printf 'ANDROID_SERIAL=%s %s\n' "${ANDROID_SERIAL:-}" "$*" >> "$FAKE_GRADLE_LOG"
SCRIPT
apply_executable "$temporary_dir/gradlew"

output="$temporary_dir/output"
set +e
FAKE_ADB_LOG="$temporary_dir/adb.log" \
FAKE_GRADLE_LOG="$temporary_dir/gradle.log" \
ANDROID_HOME="$fake_sdk" \
JAVA_HOME="$temporary_dir/java" \
SWARM_GRADLEW="$temporary_dir/gradlew" \
SWARM_TV_NAME="Bedroom TV" \
SWARM_RENDEZVOUS_URL="http://192.168.50.10:8080" \
PATH="$fake_bin:$PATH" \
    "$DEPLOY" >"$output" 2>&1 <<'CHOICE'
2
CHOICE
deploy_status=$?
set -e
if [ "$deploy_status" -ne 0 ]; then
    cat "$output" >&2
    exit "$deploy_status"
fi

grep -Fq 'Found 2 Fire TV(s) on the LAN:' "$output"
grep -Fq '1) Living Room TV | 192.168.50.20' "$output"
grep -Fq '2) Bedroom TV (preferred) | 192.168.50.30' "$output"
grep -Fq 'ANDROID_SERIAL=192.168.50.30:5555 :app:installDebug' "$temporary_dir/gradle.log"
if grep -Fq 'ANDROID_SERIAL=192.168.50.20:5555 :app:installDebug' "$temporary_dir/gradle.log"; then
    echo "Deployment ignored the selected LAN option." >&2
    exit 1
fi

echo "Fire TV deployment discovery checks passed."
