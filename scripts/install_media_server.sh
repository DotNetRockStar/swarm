#!/usr/bin/env bash
# Installs the SWARM media server (a tray app) on macOS for manual control.
#
# `run_now.sh` is a dev harness: it builds a *debug* binary into the shared
# ./target and runs it via `cargo run`, so any `cargo build`/`cargo test` in
# this workspace rebuilds and disrupts the live server (a playing TV then
# reports "server has gone offline"). It also starts a local STUN/rendezvous
# server on :8080 — needed only by TVs paired through a *swarm* (join code),
# not by TVs paired directly over the LAN.
#
# This script builds a *release* .app, installs it to /Applications (or
# ~/Applications), and installs a `swarm-server` command to ~/.local/bin.
# Nothing auto-starts or auto-restarts: you run
#   swarm-server start | stop | restart | status | logs [-f]
#
# The installed app and the dev build share
#   ~/Library/Application Support/app.swarm.server/
# (keyed by the bundle id), so the media library, LAN pairings, and settings
# carry over with no re-onboarding.
#
# LAN-only: no STUN/rendezvous server is installed. A TV paired via the LAN
# 8-digit code works directly. A TV paired via a swarm/join code needs a
# rendezvous server and will NOT reach this install — re-pair it over the LAN.
#
# Usage:
#   ./scripts/install_media_server.sh              build + install + start
#   ./scripts/install_media_server.sh --status
#   ./scripts/install_media_server.sh --uninstall  [--purge]
#
# Env vars (optional):
#   SWARM_PEER_PORT        peer QUIC port (default 8544 — matches paired TVs)
#   SWARM_HTTP_MEDIA_PORT  HTTP media port (default 8546)
#   SWARM_APP_DIR          where to install the .app (default: /Applications,
#                          falling back to ~/Applications)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="SWARM Server"
BUNDLE_ID="app.swarm.server"
BUNDLE_BIN="swarm-server-app"   # macOS exe name = Cargo bin, not product name
BUILT_APP="$REPO_ROOT/target/release/bundle/macos/$APP_NAME.app"
CMD_SRC="$REPO_ROOT/scripts/swarm-server"
CMD_DST="$HOME/.local/bin/swarm-server"
DATA_DIR="$HOME/Library/Application Support/$BUNDLE_ID"
OLD_PLIST="$HOME/Library/LaunchAgents/$BUNDLE_ID.plist"   # from earlier versions
PEER_PORT="${SWARM_PEER_PORT:-8544}"
HTTP_MEDIA_PORT="${SWARM_HTTP_MEDIA_PORT:-8546}"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "This installer is macOS-only." >&2
    exit 1
fi

# Where an existing install lives, else where a new one should go.
installed_app_path() {
    local d
    for d in "${SWARM_APP_DIR:-}" /Applications "$HOME/Applications"; do
        [ -n "$d" ] && [ -d "$d/$APP_NAME.app" ] && { echo "$d/$APP_NAME.app"; return; }
    done
    if [ -n "${SWARM_APP_DIR:-}" ]; then
        echo "$SWARM_APP_DIR/$APP_NAME.app"
    elif [ -w /Applications ]; then
        echo "/Applications/$APP_NAME.app"
    else
        echo "$HOME/Applications/$APP_NAME.app"
    fi
}
APP_PATH="$(installed_app_path)"

kill_port() {
    local port="$1" held
    held="$( { lsof -ti "tcp:$port" -sTCP:LISTEN 2>/dev/null || true; lsof -ti "udp:$port" 2>/dev/null || true; } | sort -u)"
    [ -z "$held" ] && return 0
    echo "$held" | while read -r p; do
        echo "   freed port $port (pid $p)"
        kill "$p" 2>/dev/null || true
    done
    return 0
}

detect_lan_ip() {
    local iface ip
    for iface in en0 en1 eth0; do
        ip="$(ipconfig getifaddr "$iface" 2>/dev/null)" || continue
        [ -n "$ip" ] && { echo "$ip"; return; }
    done
    echo "127.0.0.1"
}

remove_old_launchagent() {
    if [ -f "$OLD_PLIST" ] || launchctl print "gui/$(id -u)/$BUNDLE_ID" >/dev/null 2>&1; then
        echo "==> Removing the old LaunchAgent (this install is manual now) ..."
        launchctl bootout "gui/$(id -u)/$BUNDLE_ID" 2>/dev/null || true
        launchctl unload "$OLD_PLIST" 2>/dev/null || true
        rm -f "$OLD_PLIST"
        for v in SWARM_PEER_BIND SWARM_HTTP_MEDIA_BIND SWARM_FFMPEG_PATH RUST_LOG; do
            launchctl unsetenv "$v" 2>/dev/null || true
        done
    fi
}

# --- subcommands -----------------------------------------------------------

do_status() {
    if [ -x "$CMD_DST" ]; then
        SWARM_PEER_PORT="$PEER_PORT" "$CMD_DST" status
        return
    fi
    echo "swarm-server command not installed — run ./scripts/install_media_server.sh"
    if [ -d "$APP_PATH" ]; then echo "app: $APP_PATH (installed)"; else echo "app: not installed"; fi
}

do_uninstall() {
    echo "==> Removing the SWARM media server ..."
    remove_old_launchagent
    [ -x "$CMD_DST" ] && "$CMD_DST" stop 2>/dev/null || true
    if [ -f "$CMD_DST" ]; then rm -f "$CMD_DST" && echo "   removed $CMD_DST"; fi
    if [ -d "$APP_PATH" ]; then rm -rf "$APP_PATH" && echo "   removed $APP_PATH"; else echo "   no installed app"; fi
    if [ "${PURGE:-0}" = "1" ]; then
        rm -rf "$DATA_DIR" && echo "   removed $DATA_DIR (library, pairings, settings)"
    else
        echo "   kept $DATA_DIR — re-run with --purge to delete the library/pairings"
    fi
    echo "Done."
}

do_install() {
    echo "==> Preflight ..."
    command -v ffmpeg  >/dev/null 2>&1 || { echo "ffmpeg not found (brew install ffmpeg)"  >&2; exit 1; }
    command -v ffprobe >/dev/null 2>&1 || { echo "ffprobe not found (brew install ffmpeg)" >&2; exit 1; }
    command -v cargo   >/dev/null 2>&1 || { echo "cargo (Rust toolchain) required"          >&2; exit 1; }
    echo "   ffmpeg: $(command -v ffmpeg)"

    local tauri_cli="$REPO_ROOT/apps/server/node_modules/.bin/tauri"
    if [ ! -x "$tauri_cli" ]; then
        echo "==> npm ci in apps/server (Tauri CLI) ..."
        ( cd "$REPO_ROOT/apps/server" && npm ci )
    fi

    # whisper.cpp's bundled ggml uses std::filesystem (unavailable below the
    # 10.15 C++ target). tauri.conf.json pins bundle.macOS.minimumSystemVersion;
    # the cmake crate caches CMAKE_OSX_DEPLOYMENT_TARGET on its first configure,
    # so drop that crate's build dir in case an earlier build used 10.13.
    cargo clean --release -p whisper-rs-sys 2>/dev/null || true

    echo
    echo "==> Building the release app (~10-20 min the first time) ..."
    # Only the .app is needed for a local install. Skip the DMG bundle: it adds
    # minutes, needs Finder/AppleScript access, and bundle_dmg.sh fails outright
    # when a stale "SWARM Server" volume is still mounted from an earlier run.
    ( cd "$REPO_ROOT/apps/server" \
        && MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-10.15}" "$tauri_cli" build --bundles app )
    [ -d "$BUILT_APP" ] || { echo "Build did not produce $BUILT_APP" >&2; exit 1; }

    echo
    echo "==> Freeing dev-server ports (run_now.sh retires itself once they go) ..."
    local port
    for port in 8080 8543 8544 "$PEER_PORT" 8546 "$HTTP_MEDIA_PORT" 9443 3478; do
        kill_port "$port"
    done
    sleep 1

    remove_old_launchagent

    echo
    echo "==> Installing $APP_NAME.app to $(dirname "$APP_PATH") ..."
    mkdir -p "$(dirname "$APP_PATH")"
    [ -x "$CMD_DST" ] && "$CMD_DST" stop 2>/dev/null || true
    pkill -9 -f "$APP_PATH/Contents/MacOS/" 2>/dev/null || true
    sleep 1
    rm -rf "$APP_PATH"
    cp -R "$BUILT_APP" "$APP_PATH"
    xattr -dr com.apple.quarantine "$APP_PATH" 2>/dev/null || true

    echo "==> Installing the 'swarm-server' command to $CMD_DST ..."
    mkdir -p "$(dirname "$CMD_DST")"
    cp "$CMD_SRC" "$CMD_DST"
    chmod +x "$CMD_DST"

    echo "==> Starting it ..."
    SWARM_PEER_PORT="$PEER_PORT" SWARM_HTTP_MEDIA_PORT="$HTTP_MEDIA_PORT" "$CMD_DST" start || true

    local lan_ip; lan_ip="$(detect_lan_ip)"
    cat <<EOF

--------------------------------------------------------------------
$APP_NAME is installed. It does not auto-start or auto-restart.

  Control:    swarm-server {start|stop|restart|status|logs [-f]}
              (add ~/.local/bin to PATH if 'swarm-server' isn't found)
  App:        $APP_PATH
  Dashboard:  the SWARM tray icon (menu bar) -> the app window
  LAN:        $lan_ip:$PEER_PORT   (mDNS-advertised to TVs)
  Data dir:   $DATA_DIR   (library + LAN pairings carried over)

  A TV that connects fine under run_now.sh but not here is paired through
  a swarm/join code and needs the rendezvous server run_now.sh starts.
  Re-pair it over the LAN instead: on the TV forget the server, add it via
  LAN discovery (short code), approve it on the dashboard's Swarm page.

  Update after a code change:  ./scripts/install_media_server.sh
  Uninstall:                   ./scripts/install_media_server.sh --uninstall
--------------------------------------------------------------------
EOF
}

# --- args ----------------------------------------------------------------

ACTION="install"; PURGE=0
for arg in "$@"; do
    case "$arg" in
        --status)    ACTION="status" ;;
        --uninstall) ACTION="uninstall" ;;
        --purge)     PURGE=1 ;;
        -h|--help)   sed -n '2,36p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)           echo "Unknown argument: $arg (see --help)" >&2; exit 1 ;;
    esac
done

case "$ACTION" in
    status)    do_status ;;
    uninstall) do_uninstall ;;
    install)   do_install ;;
esac
