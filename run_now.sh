#!/usr/bin/env bash
# Runs a real SWARM STUN server + a real media server locally, for manual
# testing — the same two binaries every automated test in this repo spawns
# as subprocesses, just left running so you can point a browser, curl, or a
# real device (Fire TV, phone, whatever) at them yourself. Ctrl+C stops
# both cleanly.
#
# Binds 0.0.0.0, not just loopback: a real device on the same Wi-Fi/LAN
# needs to reach these, not just this machine. Use the "LAN" URL printed
# below (not the "local" one) in the Fire TV client's passcode-entry
# screen and in your browser if you're setting the account up from a
# different machine.
#
# First run: open the STUN server's web UI (URL printed below), create an
# account and a swarm, and mint a join code. Then either:
#   - paste the code into the Tauri GUI:
#       cargo run -p swarm-server --features gui --bin swarm-server-app
#   - or stop this script and re-run with the code so the headless daemon
#     auto-registers on startup (see SWARM_STUN_CODE below).
#
# Env vars (all optional):
#   SWARM_STUN_PORT   STUN server HTTP port (default 8080)
#   SWARM_PEER_PORT   media server's peer QUIC port (default 8543)
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
cleanup() {
    echo
    echo "Stopping..."
    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

echo "==> Building swarm-stun-server + swarm-serverd (debug)..."
cargo build --bin swarm-stun-server --bin swarm-serverd

echo "==> Starting STUN server on 0.0.0.0:$STUN_PORT ..."
SWARM_DATABASE_PATH="$RUN_DIR/stun-data/swarm.sqlite" \
SWARM_HTTP_BIND="0.0.0.0:$STUN_PORT" \
SWARM_PUBLIC_URL="http://$LAN_IP:$STUN_PORT" \
    cargo run -q --bin swarm-stun-server &
pids+=($!)

echo "==> Starting media server (peer QUIC on 0.0.0.0:$PEER_PORT, media root $RUN_DIR/media) ..."
SWARM_MEDIA_ROOT="$RUN_DIR/media" \
SWARM_DATA_DIR="$RUN_DIR/server-data" \
SWARM_PEER_BIND="0.0.0.0:$PEER_PORT" \
    cargo run -q --bin swarm-serverd &
pids+=($!)

cat <<EOF

--------------------------------------------------------------------
SWARM is running, reachable from this machine and from your LAN:
  local  http://127.0.0.1:$STUN_PORT   (browser on this machine, Swagger at /api/docs)
  LAN    http://$LAN_IP:$STUN_PORT   <- use this one in the Fire TV client / other devices

  Media server: peer QUIC on port $PEER_PORT (both addresses above)
                drop files into $RUN_DIR/media to serve them

First time here: open the STUN URL above, create an account and a
swarm, and mint a join code. Then link the media server to it —
either through the Tauri GUI, or by stopping this script (Ctrl+C) and
re-running with:

  SWARM_STUN_URL=http://$LAN_IP:$STUN_PORT SWARM_STUN_CODE=<code> ./run_now.sh

Ctrl+C to stop both servers.
--------------------------------------------------------------------
EOF

wait
