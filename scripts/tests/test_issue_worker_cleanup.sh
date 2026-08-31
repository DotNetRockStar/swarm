#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CRON="$REPO_ROOT/scripts/tests/full_uat_cron.sh"
RATE_LIMITS="$REPO_ROOT/scripts/tests/codex_rate_limits.py"

legacy_paths=(
    "$REPO_ROOT/scripts/issue_worker"
    "$REPO_ROOT/scripts/install_swarm_issue_cron.sh"
    "$REPO_ROOT/scripts/swarm_issue_worker.sh"
    "$REPO_ROOT/scripts/test_swarm_issue_worker.sh"
)
for legacy_path in "${legacy_paths[@]}"; do
    if [ -e "$legacy_path" ]; then
        echo "Legacy issue-worker path still exists: $legacy_path" >&2
        exit 1
    fi
done

bash -n "$CRON"
python3 -m py_compile "$RATE_LIMITS"
grep -Fq 'scripts/tests/codex_rate_limits.py' "$CRON"
if grep -Fq 'scripts/issue_worker/' "$CRON"; then
    echo "Full UAT cron still depends on the removed issue worker." >&2
    exit 1
fi

temporary_dir="$(mktemp -d)"
trap 'rm -rf "$temporary_dir"' EXIT
fake_codex="$temporary_dir/codex"
cat > "$fake_codex" <<'PYTHON'
#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    if request.get("method") == "initialize":
        print(json.dumps({"id": request["id"], "result": {}}), flush=True)
    elif request.get("method") == "account/rateLimits/read":
        limits = {
            "primary": {"usedPercent": 12.5},
            "secondary": {"usedPercent": 25},
            "rateLimitReachedType": None,
            "spendControlReached": False,
        }
        print(json.dumps({"id": request["id"], "result": {"rateLimits": limits}}), flush=True)
PYTHON
chmod +x "$fake_codex"

actual="$(python3 "$RATE_LIMITS" --codex-bin "$fake_codex" --timeout 2)"
expected='{"primary":{"usedPercent":12.5},"rateLimitReachedType":null,"secondary":{"usedPercent":25},"spendControlReached":false}'
if [ "$actual" != "$expected" ]; then
    echo "Unexpected Codex rate-limit response: $actual" >&2
    exit 1
fi

echo "Issue-worker cleanup integration checks passed."
