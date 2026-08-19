#!/usr/bin/env bash
# Runs a real SWARM STUN server + a real headless media server + the Tauri
# media server GUI, all locally, for manual testing — the first two are the
# same binaries every automated test in this repo spawns as subprocesses,
# just left running so you can point a browser, curl, or a real device
# (Fire TV, phone, whatever) at them yourself; the GUI is the same
# ServerCore wrapped in a window, for visually checking onboarding/library/
# swarm state instead of only curl+logs. Ctrl+C stops all three, and
# actually stops them — see the cleanup() note below on why that isn't
# as trivial as it sounds with `cargo run` in the mix.
#
# Binds 0.0.0.0, not just loopback: a real device on the same Wi-Fi/LAN
# needs to reach these, not just this machine. Use the "LAN" URL printed
# below (not the "local" one) in the Fire TV client's passcode-entry
# screen and in your browser if you're setting the account up from a
# different machine.
#
# First run: open the STUN server's web UI (URL printed below), create an
# account and a swarm, and mint a join code. Then either:
#   - paste the code into the GUI window this script already opened, or
#   - stop this script and re-run with the code so the headless daemon
#     auto-registers on startup (see SWARM_STUN_CODE below).
#
# Env vars (all optional):
#   SWARM_STUN_PORT      STUN server HTTP port (default 8080)
#   SWARM_PEER_PORT      headless media server's peer QUIC port (default 8543)
#   SWARM_GUI_PEER_PORT  GUI media server's peer QUIC port (default 8544) —
#                        must differ from SWARM_PEER_PORT since both are
#                        real ServerCore instances bound at the same time
#   SWARM_RUN_DIR     where local state (sqlite dbs, media root) lives (default .run)
#   SWARM_STUN_URL / SWARM_STUN_CODE   set both to auto-register the media
#                     server into a swarm on startup (see main.rs)
#   RUST_LOG          default "info"

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

if [ -d "$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin" ]; then
    export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
fi

RUN_DIR="${SWARM_RUN_DIR:-.run}"
STUN_PORT="${SWARM_STUN_PORT:-8080}"
PEER_PORT="${SWARM_PEER_PORT:-8543}"
GUI_PEER_PORT="${SWARM_GUI_PEER_PORT:-8544}"
# The STUN server's own reflector, not otherwise surfaced here — needed so
# cleanup() can free them too; keep in sync with config.rs's default if that
# ever changes.
REFLECTOR_PORTS="9443 3478"
export RUST_LOG="${RUST_LOG:-info}"

mkdir -p "$RUN_DIR/stun-data" "$RUN_DIR/server-data" "$RUN_DIR/media"

# Prefers a real network interface over a VPN tunnel's virtual address.
# swarm_p2p::local_addr's routing-table-probe trick (UDP "connect" to a
# well-known address, no packet actually sent) is what the real Rust
# server uses to self-report peer_addr, and it has the same blind spot
# this would have if it were tried first here: a full-tunnel VPN (common —
# Tailscale, corporate VPN, WireGuard) captures the default route, so the
# probe reports the VPN's internal address, not one a device on the same
# physical LAN can actually reach. Asking named interfaces directly sidesteps
# that ambiguity; the probe is only a fallback for when none of them exist.
detect_lan_ip() {
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
LAN_IP="$(detect_lan_ip)"

pids=()
# Kills whatever's actually listening on $1 (tcp) — belt-and-suspenders
# alongside the tracked-pid kill below. Confirmed necessary the hard way:
# `cargo run ... &` backgrounds cargo's own pid, but cargo can exit once its
# child binary is up, reparenting the real server process — killing only
# the tracked pid then does nothing, and the binary keeps running (and
# holding its port) indefinitely, invisibly, until something notices the
# *next* run_now.sh fails to bind with "Address already in use". Killing by
# port is immune to exactly how cargo happens to structure that process tree.
kill_port() {
    # `[ cond ] && cmd` as a bare statement returns cond's (non-zero,
    # "false") exit status whenever cond is false — under `set -e`, that's
    # indistinguishable from a real failure and kills the whole script the
    # first time this runs against a genuinely-free port. `if`/`fi` doesn't
    # have that problem (an `if` statement's own exit status is always 0
    # regardless of which branch ran), and the explicit `return 0` makes it
    # robust even if a future edit adds a command after the `if` blocks.
    local port="$1"
    local held_by
    held_by="$(lsof -ti "tcp:$port" -sTCP:LISTEN 2>/dev/null || true)"
    if [ -n "$held_by" ]; then
        echo "$held_by" | while read -r p; do kill "$p" 2>/dev/null || true; done
    fi
    held_by="$(lsof -ti "udp:$port" 2>/dev/null || true)"
    if [ -n "$held_by" ]; then
        echo "$held_by" | while read -r p; do kill "$p" 2>/dev/null || true; done
    fi
    return 0
}
cleanup() {
    echo
    echo "Stopping..."
    # `${pids[@]}` alone, under `set -u`, is a real bash 3.2 bug (macOS's
    # default /bin/bash) — it raises "unbound variable" for a *zero-length*
    # array even though `pids` is legitimately declared, not actually
    # unset. `${pids[@]:-}` sidesteps it as an empty expansion instead.
    # Reachable for real: the pre-flight kill_port pass below now runs
    # before anything is ever appended to pids.
    for pid in "${pids[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    sleep 0.3
    for port in "$STUN_PORT" "$PEER_PORT" "$GUI_PEER_PORT" $REFLECTOR_PORTS; do
        kill_port "$port"
    done
}
trap cleanup EXIT INT TERM

# Self-healing pre-flight: a previous run that didn't exit through this
# script's own cleanup() (terminal closed, machine slept, `kill -9`, a
# crash) leaves these ports held by orphaned processes — the next
# ./run_now.sh would otherwise fail to bind with "Address already in use"
# instead of just working. Same kill_port() cleanup() already uses on the
# way out, run once on the way in too.
echo "==> Checking for already-running SWARM processes on our ports..."
for port in "$STUN_PORT" "$PEER_PORT" "$GUI_PEER_PORT" $REFLECTOR_PORTS; do
    kill_port "$port"
done

echo "==> Building swarm-stun-server + swarm-serverd + the GUI (debug)..."
cargo build --bin swarm-stun-server --bin swarm-serverd
cargo build -p swarm-server --features gui --bin swarm-server-app

echo "==> Starting STUN server on 0.0.0.0:$STUN_PORT ..."
# swarm-stun-server defaults SWARM_STATIC_DIR to the relative path "static",
# resolved against the process's cwd — which `cargo run` leaves as wherever
# it was invoked from (this script's repo root), not the crate's own
# directory. Left unset, that relative path silently misses the real UI at
# apps/stun-server/static, and the server falls back to API-only (every
# non-API path, including "/", 404s) with only a log warning to show for it.
SWARM_DATABASE_PATH="$RUN_DIR/stun-data/swarm.sqlite" \
SWARM_HTTP_BIND="0.0.0.0:$STUN_PORT" \
SWARM_PUBLIC_URL="http://$LAN_IP:$STUN_PORT" \
SWARM_STATIC_DIR="apps/stun-server/static" \
    cargo run -q --bin swarm-stun-server &
pids+=($!)

# Real startup-order race, not theoretical: with SWARM_STUN_CODE set for
# auto-registration, swarm-serverd's one registration attempt at startup
# fired before swarm-stun-server had actually bound its listener yet
# (both processes start ~simultaneously as background jobs) — a silent,
# non-fatal "could not reach STUN server" that left the media server
# running but unregistered, with no automatic retry. Block on real
# readiness instead of guessing a sleep duration.
echo "==> Waiting for the STUN server to be ready ..."
for _ in $(seq 1 50); do
    curl -s -o /dev/null "http://127.0.0.1:$STUN_PORT/health" && break
    sleep 0.2
done

echo "==> Starting media server (peer QUIC on 0.0.0.0:$PEER_PORT, media root $RUN_DIR/media) ..."
SWARM_MEDIA_ROOT="$RUN_DIR/media" \
SWARM_DATA_DIR="$RUN_DIR/server-data" \
SWARM_PEER_BIND="0.0.0.0:$PEER_PORT" \
    cargo run -q --bin swarm-serverd &
pids+=($!)

echo "==> Opening the media server GUI (peer QUIC on 0.0.0.0:$GUI_PEER_PORT) ..."
SWARM_PEER_BIND="0.0.0.0:$GUI_PEER_PORT" \
    cargo run -q -p swarm-server --features gui --bin swarm-server-app &
pids+=($!)

cat <<EOF

--------------------------------------------------------------------
SWARM is running, reachable from this machine and from your LAN:
  local  http://127.0.0.1:$STUN_PORT   (browser on this machine, Swagger at /api/docs)
  LAN    http://$LAN_IP:$STUN_PORT   <- use this one in the Fire TV client / other devices

  Headless media server: peer QUIC on port $PEER_PORT (both addresses above)
                         drop files into $RUN_DIR/media to serve them
  GUI media server:      a separate window should now be open, peer QUIC on
                         port $GUI_PEER_PORT — pick its own media folder there

First time here: open the STUN URL above, create an account and a
swarm, and mint a join code. Then link a media server to it — paste
the code into the GUI window this script just opened, or stop this
script (Ctrl+C) and re-run with:

  SWARM_STUN_URL=http://$LAN_IP:$STUN_PORT SWARM_STUN_CODE=<code> ./run_now.sh

(that auto-registers the headless server only — the GUI always joins
via its own window)

Ctrl+C to stop everything — the STUN server, both media servers, and
the GUI window all shut down together.
--------------------------------------------------------------------
EOF

wait
