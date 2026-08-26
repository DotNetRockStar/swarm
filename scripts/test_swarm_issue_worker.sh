#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3)}"
exec "$PYTHON_BIN" "$SCRIPT_DIR/issue_worker/test_swarm_issue_worker.py" "$@"
