#!/usr/bin/env bash

# Backward-compatible launcher for the Python foreground scheduler.
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3)}"
exec "$PYTHON_BIN" "$SCRIPT_DIR/issue_worker/install_swarm_issue_cron.py" "$@"
