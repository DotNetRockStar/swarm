#!/usr/bin/env bash

# Backward-compatible launcher. The worker implementation is Python; existing
# cron commands and environment-variable based installations may keep calling
# this path during migration.
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3)}"
exec "$PYTHON_BIN" "$SCRIPT_DIR/issue_worker/swarm_issue_worker.py" "$@"
