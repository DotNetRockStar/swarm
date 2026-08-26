#!/usr/bin/env bash

# Launch the loopback-only GitHub bot setup UI and keep it attached to this
# terminal. setup_github_bots.py opens the default browser; Ctrl+C stops both
# the setup server and this launcher.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3)}"

exec "$PYTHON_BIN" "$SCRIPT_DIR/setup_github_bots.py" "$@"
