#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SUITE="$REPO_ROOT/scripts/tests/tv_uat_suite.sh"

bash -n "$SUITE"
grep -q '^POST_GITHUB_ISSUE=0$' "$SUITE"
grep -q -- '--github-issue) POST_GITHUB_ISSUE=1' "$SUITE"
grep -q '\[ "$POST_GITHUB_ISSUE" -eq 1 \]' "$SUITE"

output_file="$(mktemp)"
trap 'rm -f "$output_file"' EXIT
if "$SUITE" --github-issue --no-issue >"$output_file" 2>&1; then
    echo "Expected conflicting reporting flags to fail." >&2
    exit 1
fi
grep -q -- '--github-issue and --no-issue cannot be used together' "$output_file"

echo "TV UAT reporting option checks passed."
