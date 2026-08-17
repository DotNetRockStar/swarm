#!/usr/bin/env bash
# Runs a real SWARM STUN server + a real media server locally, for manual
# testing — the same two binaries every automated test in this repo spawns
# as subprocesses, just left running so you can point a browser, curl, or
# (eventually) a real client at them yourself. Ctrl+C stops both cleanly.
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

echo "==> Starting STUN server on http://127.0.0.1:$STUN_PORT ..."
SWARM_DATABASE_PATH="$RUN_DIR/stun-data/swarm.sqlite" \
SWARM_HTTP_BIND="127.0.0.1:$STUN_PORT" \
SWARM_PUBLIC_URL="http://127.0.0.1:$STUN_PORT" \
    cargo run -q --bin swarm-stun-server &
pids+=($!)

echo "==> Starting media server (peer QUIC on 127.0.0.1:$PEER_PORT, media root $RUN_DIR/media) ..."
SWARM_MEDIA_ROOT="$RUN_DIR/media" \
SWARM_DATA_DIR="$RUN_DIR/server-data" \
SWARM_PEER_BIND="127.0.0.1:$PEER_PORT" \
    cargo run -q --bin swarm-serverd &
pids+=($!)

cat <<EOF

--------------------------------------------------------------------
SWARM is running locally:
  STUN server   http://127.0.0.1:$STUN_PORT   (Swagger UI: /api/docs)
  Media server  peer QUIC on 127.0.0.1:$PEER_PORT
                drop files into $RUN_DIR/media to serve them

First time here: open http://127.0.0.1:$STUN_PORT , create an account
and a swarm, and mint a join code. Then link the media server to it —
either through the Tauri GUI, or by stopping this script (Ctrl+C) and
re-running with:

  SWARM_STUN_URL=http://127.0.0.1:$STUN_PORT SWARM_STUN_CODE=<code> ./run_now.sh

Ctrl+C to stop both servers.
--------------------------------------------------------------------
EOF

wait
