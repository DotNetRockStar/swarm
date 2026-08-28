#!/usr/bin/env bash
# See scripts/TV_TESTING.md for how this fits alongside tv_e2e_suite.sh and
# tv_uat_suite.sh as part of "run everything."
#
# Backend UAT suite for the media server: real `#[tauri::command]` handlers
# (apps/server/src/gui.rs) invoked directly against a real, isolated
# AppState — real SQLite, real filesystem, a real ServerCore with its own
# unique QUIC/HTTP ports per test — behind a mocked Tauri runtime. See
# apps/server/src/gui_tests/mod.rs's doc comment for why this shape (no
# reliable macOS UI-automation path today; Tauri's simulated IPC/ACL layer
# isn't usable under a bare mock_context()).
#
# Unlike tv_e2e_suite.sh/tv_uat_suite.sh, this suite needs no real Fire TV,
# no LAN, and doesn't touch the already-running desktop server — it's a
# plain `cargo test` run and is CI-friendly (no local hardware dependency).
# It covers backend/API correctness only; real user-visible UI flows still
# need the TV-side suites or a human.
#
# This suite's test logic is change-controlled the same as the TV suites —
# read the swarm-e2e-suite-lockdown skill before editing scenario logic in
# apps/server/src/gui_tests/. Genuine infra bugs (a flaky test, a real
# product bug the tests surface) are fair game to fix without asking.
#
# Usage:
#   ./scripts/media_server_uat_tests.sh                # run every backend UAT test
#   ./scripts/media_server_uat_tests.sh media_root      # run only tests whose name contains this substring
#
# Exit code: 0 only if every test passed; whatever `cargo test` returns
# otherwise (nonzero). Safe to chain in CI or a script.

set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

FILTER="${1:-}"

echo "=== Media server backend UAT tests ==="
if [ -n "$FILTER" ]; then
    echo "Filter: $FILTER"
fi

cargo test -p swarm-server --bin swarm-server-app --features gui -- "$FILTER"
STATUS=$?

if [ "$STATUS" -eq 0 ]; then
    echo "=== Media server backend UAT tests: PASS ==="
else
    echo "=== Media server backend UAT tests: FAIL (exit $STATUS) ==="
fi
exit "$STATUS"
