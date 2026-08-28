#!/usr/bin/env bash
# Opt-in real-TV transport resilience journey. It reuses the UAT suite's
# targeting, testing-mode authorization, reporting, and evidence collection,
# but remains separate because deliberately dropping live client transports
# is slower and more disruptive than the default functional UAT catalog.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$SCRIPT_DIR/tv_uat_suite.sh" --test ResilienceUatTest "$@"
