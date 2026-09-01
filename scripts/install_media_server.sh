#!/usr/bin/env bash
# Installs the SWARM media server as an always-on macOS background service.
#
# `run_now.sh` is a *dev harness*: it builds a debug binary into the shared
# ./target/debug, runs it via `cargo run`, and also starts a local STUN
# server. Any `cargo build`/`cargo test` in this workspace then rebuilds that
# same binary and saturates the CPU, so the live server drops off the LAN for
# tens of seconds and a playing TV reports "server has gone offline". This
# script instead builds a *release* .app, installs it under ~/Applications
# (outside ./target, so repo builds never touch it), and registers a
# LaunchAgent that starts it at login and keeps it alive (KeepAlive=true) on
# the port your TVs are paired to. A "Quit SWARM" from the tray is relaunched
# within seconds; to actually stop the service run
#   launchctl bootout gui/$UID/app.swarm.server
#
# The installed app and the dev build share
#   ~/Library/Application Support/app.swarm.server/
# (keyed by the bundle identifier), so the media library, TV pairings, and
# settings carry over with no re-onboarding.
#
# LAN-only: no STUN/rendezvous server is installed. Pairing over the LAN (the
# 8-digit code shown on the TV) still works. Internet/remote access would need
# a public rendezvous URL compiled in — not handled here.
#
# Usage:
#   ./scripts/install_media_server.sh              install or update + restart
#   ./scripts/install_media_server.sh --status     show service state
#   ./scripts/install_media_server.sh --uninstall  remove the service + app
#   ./scripts/install_media_server.sh --uninstall --purge   also delete the data dir
#
# Env vars (all optional):
#   SWARM_PEER_PORT   peer QUIC port to bind (default 8544 — matches run_now.sh
#                     and what already-paired TVs expect)
#   SWARM_HTTP_MEDIA_PORT   HTTP media port (default 8546)
#   SWARM_APP_DIR     where to install the .app (default ~/Applications)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="SWARM Server"
# The macOS bundle executable is the Cargo bin name, not the product name.
BUNDLE_BIN="swarm-server-app"
BUNDLE_ID="app.swarm.server"
LABEL="$BUNDLE_ID"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
# Install to ~/Applications by default (no admin prompt), but adopt an
# existing install wherever it already lives (e.g. a manual drag to /Applications).
if [ -d "/Applications/$APP_NAME.app" ] && [ -z "${SWARM_APP_DIR:-}" ]; then
    APP_DIR="/Applications"
else
    APP_DIR="${SWARM_APP_DIR:-$HOME/Applications}"
fi
APP_PATH="$APP_DIR/$APP_NAME.app"
BUILT_APP="$REPO_ROOT/target/release/bundle/macos/$APP_NAME.app"

# Resolve the bundle executable from Info.plist (falls back to the known bin
# name). Used for the LaunchAgent Program and process matching.
resolve_executable() {
    local app="$1" name
    name="$(defaults read "$app/Contents/Info.plist" CFBundleExecutable 2>/dev/null || true)"
    [ -n "$name" ] || name="$BUNDLE_BIN"
    printf '%s/Contents/MacOS/%s' "$app" "$name"
}
EXECUTABLE="$(resolve_executable "$APP_PATH")"
PEER_PORT="${SWARM_PEER_PORT:-8544}"
HTTP_MEDIA_PORT="${SWARM_HTTP_MEDIA_PORT:-8546}"
OUT_LOG="$HOME/Library/Logs/swarm-media-server.out.log"
ERR_LOG="$HOME/Library/Logs/swarm-media-server.err.log"
DATA_DIR="$HOME/Library/Application Support/$BUNDLE_ID"
DOMAIN="gui/$(id -u)"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "This installer is macOS-only. On Linux/Windows, run the server from a" >&2
    echo "release build under a systemd unit / Task Scheduler entry instead." >&2
    exit 1
fi

# --- shared helpers -------------------------------------------------------

# Kills whatever is listening on a TCP or bound to a UDP port. Same approach
# run_now.sh uses: `cargo run` can exit once its child is up, reparenting the
# real server, so killing a tracked pid is not enough — kill by port.
kill_port() {
    local port="$1" held
    held="$(lsof -ti "tcp:$port" -sTCP:LISTEN 2>/dev/null || true)"
    if [ -n "$held" ]; then
        echo "$held" | while read -r p; do
            echo "   stopped pid $p (tcp:$port)"
            kill "$p" 2>/dev/null || true
        done
    fi
    held="$(lsof -ti "udp:$port" 2>/dev/null || true)"
    if [ -n "$held" ]; then
        echo "$held" | while read -r p; do
            echo "   stopped pid $p (udp:$port)"
            kill "$p" 2>/dev/null || true
        done
    fi
    return 0
}

# Prefers a real interface over a VPN tunnel's virtual address (see the long
# note in run_now.sh's copy).
detect_lan_ip() {
    local iface ip
    for iface in en0 en1 eth0; do
        ip="$(ipconfig getifaddr "$iface" 2>/dev/null)" || continue
        if [ -n "$ip" ]; then
            echo "$ip"
            return
        fi
    done
    python3 -c "
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try:
    s.connect(('8.8.8.8', 80))
    print(s.getsockname()[0])
except Exception:
    raise SystemExit(1)
finally:
    s.close()
" 2>/dev/null || echo "127.0.0.1"
}

launchctl_stop() {
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null \
        || launchctl unload "$PLIST" 2>/dev/null \
        || true
}

launchctl_start() {
    # Also publish the bind/ffmpeg vars session-wide so an app launched from
    # Finder/Dock (not just this LaunchAgent) still uses the paired port. This
    # is session-scoped; the LaunchAgent's own EnvironmentVariables are the
    # durable copy.
    launchctl setenv SWARM_PEER_BIND "0.0.0.0:$PEER_PORT" 2>/dev/null || true
    launchctl setenv SWARM_HTTP_MEDIA_BIND "0.0.0.0:$HTTP_MEDIA_PORT" 2>/dev/null || true
    [ -n "${ffmpeg_bin:-}" ] && launchctl setenv SWARM_FFMPEG_PATH "$ffmpeg_bin" 2>/dev/null || true
    launchctl bootstrap "$DOMAIN" "$PLIST" 2>/dev/null \
        || launchctl load "$PLIST" 2>/dev/null \
        || true
    launchctl kickstart -k "$DOMAIN/$LABEL" 2>/dev/null || true
}

service_pid() {
    # `launchctl print gui/<uid>/<label>` includes a `pid = <n>` line while the
    # job is running. macOS awk has no \s, so match literally.
    launchctl print "$DOMAIN/$LABEL" 2>/dev/null \
        | sed -n 's/^[[:space:]]*pid = \([0-9][0-9]*\).*/\1/p' \
        | head -n1
}

# --- subcommands --------------------------------------------------------

do_status() {
    echo "SWARM media server service status"
    echo "  label:        $LABEL"
    echo "  app:          $APP_PATH"
    if [ -x "$EXECUTABLE" ]; then
        local ver
        ver="$(defaults read "$APP_PATH/Contents/Info.plist" CFBundleShortVersionString 2>/dev/null || echo '?')"
        echo "  installed:    yes (version $ver)"
    else
        echo "  installed:    no"
    fi
    if [ -f "$PLIST" ]; then
        echo "  launchagent:  $PLIST"
        local pid
        pid="$(service_pid || true)"
        if [ -n "${pid:-}" ] && [ "$pid" != "0" ]; then
            echo "  running:      yes (pid $pid)"
            ps -o etime=,command= -p "$pid" 2>/dev/null | sed 's/^/    /'
        else
            echo "  running:      no"
        fi
    else
        echo "  launchagent:  not installed"
    fi
    local port_pids
    port_pids="$( { lsof -ti "tcp:$PEER_PORT" -sTCP:LISTEN 2>/dev/null; lsof -ti "udp:$PEER_PORT" 2>/dev/null; } | sort -u | tr '\n' ' ' | sed 's/ $//' )"
    if [ -n "$port_pids" ]; then
        echo "  peer port:    $PEER_PORT  (held by pid $port_pids)"
    else
        echo "  peer port:    $PEER_PORT  (free)"
    fi
    local today_log="$DATA_DIR/logs/server.log.$(date -u +%Y-%m-%d)"
    if [ -f "$today_log" ]; then
        echo "  --- last server log lines ---"
        tail -n 6 "$today_log" 2>/dev/null | cut -c1-150 | sed 's/^/    /'
    fi
}

do_uninstall() {
    echo "==> Stopping and removing the SWARM media server service ..."
    launchctl_stop
    if [ -f "$PLIST" ]; then
        rm -f "$PLIST" && echo "   removed $PLIST"
    else
        echo "   no LaunchAgent to remove"
    fi
    if [ -d "$APP_PATH" ]; then
        rm -rf "$APP_PATH" && echo "   removed $APP_PATH"
    else
        echo "   no installed app to remove"
    fi
    if [ "${PURGE:-0}" = "1" ]; then
        rm -rf "$DATA_DIR" && echo "   removed $DATA_DIR (library, pairings, settings)"
    else
        echo "   kept $DATA_DIR (re-run with --purge to delete the library/pairings)"
    fi
    echo "Done."
}

do_install() {
    echo "==> Preflight ..."

    local ffmpeg_bin ffprobe_bin
    ffmpeg_bin="$(command -v ffmpeg || true)"
    ffprobe_bin="$(command -v ffprobe || true)"
    if [ -z "$ffmpeg_bin" ] || [ -z "$ffprobe_bin" ]; then
        echo "ffmpeg and ffprobe must be installed (e.g. 'brew install ffmpeg')." >&2
        echo "The media server needs both to probe and transcode media." >&2
        exit 1
    fi
    local ffmpeg_dir
    ffmpeg_dir="$(cd "$(dirname "$ffmpeg_bin")" && pwd)"
    echo "   ffmpeg:  $ffmpeg_bin"
    echo "   ffprobe: $ffprobe_bin"

    if ! command -v cargo >/dev/null 2>&1; then
        echo "A Rust toolchain (cargo) is required to build the app." >&2
        exit 1
    fi

    local tauri_cli="$REPO_ROOT/apps/server/node_modules/.bin/tauri"
    if [ ! -x "$tauri_cli" ]; then
        echo "==> Installing the Tauri CLI (npm ci in apps/server) ..."
        ( cd "$REPO_ROOT/apps/server" && npm ci )
    fi

    # whisper.cpp's bundled ggml uses std::filesystem, unavailable below the
    # 10.15 C++ deployment target. tauri.conf.json's bundle.macOS.minimumSystemVersion
    # pins this, and MACOSX_DEPLOYMENT_TARGET below covers a plain cargo build —
    # but the cmake crate caches CMAKE_OSX_DEPLOYMENT_TARGET in its build dir on
    # the first configure, so an earlier build at the wrong target sticks. Drop
    # just that crate's artifacts so it re-configures.
    cargo clean --release -p whisper-rs-sys 2>/dev/null || true

    echo
    echo "==> Building the release app — this takes ~10-20 minutes the first time ..."
    ( cd "$REPO_ROOT/apps/server" && MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-10.15}" "$tauri_cli" build )
    if [ ! -d "$BUILT_APP" ]; then
        echo "Build did not produce $BUILT_APP" >&2
        exit 1
    fi

    echo
    echo "==> Handing over from the dev server (run_now.sh) ..."
    # Kill by port, not by matching run_now.sh: a stale automation dev server
    # is also `scripts/run_now.sh`. run_now.sh's own `wait` returns once its
    # server/STUN children are gone, and its EXIT trap then cleans up — so
    # freeing the ports is enough to retire it. Ports: STUN 8080, peer 8543 +
    # 8544, HTTP media 8546, STUN reflectors 9443 + 3478.
    local port
    for port in 8080 8543 8544 "$PEER_PORT" 8546 "$HTTP_MEDIA_PORT" 9443 3478; do
        kill_port "$port"
    done
    sleep 1

    echo
    echo "==> Installing $APP_NAME.app to $APP_DIR ..."
    mkdir -p "$APP_DIR"
    launchctl_stop
    rm -rf "$APP_PATH"
    cp -R "$BUILT_APP" "$APP_PATH"
    # Locally-built bundles have no com.apple.quarantine attr, but strip it
    # anyway in case the app dir is a synced/attributed volume.
    xattr -dr com.apple.quarantine "$APP_PATH" 2>/dev/null || true
    # Re-resolve against the freshly installed bundle's Info.plist.
    EXECUTABLE="$(resolve_executable "$APP_PATH")"
    if [ ! -x "$EXECUTABLE" ]; then
        echo "Installed bundle has no executable at $EXECUTABLE" >&2
        exit 1
    fi

    echo "==> Writing the LaunchAgent ($PLIST) ..."
    mkdir -p "$(dirname "$PLIST")" "$HOME/Library/Logs"
    cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>Program</key>
    <string>$EXECUTABLE</string>
    <key>RunAtLoad</key>
    <true/>
    <!-- Always relaunch. The app has tauri-plugin-single-instance, so a Finder
         launch just forwards to this one; a "Quit SWARM" from the tray is
         relaunched here on the right port within seconds. To actually stop the
         service: launchctl bootout gui/$UID/app.swarm.server -->
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SWARM_PEER_BIND</key>
        <string>0.0.0.0:$PEER_PORT</string>
        <key>SWARM_HTTP_MEDIA_BIND</key>
        <string>0.0.0.0:$HTTP_MEDIA_PORT</string>
        <key>SWARM_FFMPEG_PATH</key>
        <string>$ffmpeg_bin</string>
        <key>PATH</key>
        <string>$ffmpeg_dir:/usr/bin:/bin:/usr/sbin:/sbin</string>
        <key>RUST_LOG</key>
        <string>info</string>
    </dict>
    <key>StandardOutPath</key>
    <string>$OUT_LOG</string>
    <key>StandardErrorPath</key>
    <string>$ERR_LOG</string>
</dict>
</plist>
PLIST_EOF

    echo "==> Starting the service ..."
    launchctl_start
    sleep 2

    local pid lan_ip
    pid="$(service_pid || true)"
    lan_ip="$(detect_lan_ip)"

    echo
    echo "--------------------------------------------------------------------"
    if [ -n "${pid:-}" ] && [ "$pid" != "0" ]; then
        echo "SWARM media server is installed and running (pid $pid)."
    else
        echo "SWARM media server is installed. It did not report a pid yet —"
        echo "check:  ./scripts/install_media_server.sh --status"
        echo "        tail -f \"$ERR_LOG\""
    fi
    echo
    echo "  App:        $APP_PATH"
    echo "  Dashboard:  click the SWARM tray icon (top menu bar) -> the app window"
    echo "  LAN peer:   $lan_ip:$PEER_PORT   (mDNS-advertised to TVs automatically)"
    echo "  Data dir:   $DATA_DIR   (library + pairings carried over from run_now.sh)"
    echo "  Logs:       $DATA_DIR/logs/   (structured)   |   $ERR_LOG (stderr)"
    echo
    echo "  First time: open the dashboard from the tray, confirm the media folder"
    echo "  is set and the TV still shows as paired. It should — same data dir,"
    echo "  same port. If no tray icon appears within a few seconds, run"
    echo "    open \"$APP_PATH\""
    echo "  once (launchd keeps it alive afterwards)."
    echo
    echo "  Stop the service:   launchctl bootout $DOMAIN/$LABEL"
    echo "  Start again:        launchctl bootstrap $DOMAIN \"$PLIST\""
    echo "  Update after a code change:   ./scripts/install_media_server.sh"
    echo "  Uninstall:          ./scripts/install_media_server.sh --uninstall"
    echo
    echo "  Do not use run_now.sh for the living-room server any more — cargo"
    echo "  builds in this repo no longer affect the installed app."
    echo "--------------------------------------------------------------------"
}

# --- arg parsing --------------------------------------------------------

ACTION="install"
PURGE=0
for arg in "$@"; do
    case "$arg" in
        --status)    ACTION="status" ;;
        --uninstall) ACTION="uninstall" ;;
        --purge)     PURGE=1 ;;
        -h|--help)
            sed -n '2,32p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg (see --help)" >&2
            exit 1
            ;;
    esac
done

case "$ACTION" in
    status)    do_status ;;
    uninstall) do_uninstall ;;
    install)   do_install ;;
esac
