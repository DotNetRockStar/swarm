#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/swarm-issue-worker-test.XXXXXX")"
TEST_REPO="$TEST_DIR/repo"
TEST_STATE="$TEST_DIR/state"

cleanup_test() {
    rm -rf -- "$TEST_DIR"
}
trap cleanup_test EXIT

mkdir -p -- "$TEST_REPO" "$TEST_STATE"
git -C "$TEST_REPO" init -q -b main
git -C "$TEST_REPO" config user.name "SWARM worker test"
git -C "$TEST_REPO" config user.email "worker-test@example.invalid"
printf 'base\n' > "$TEST_REPO/tracked.txt"
git -C "$TEST_REPO" add tracked.txt
git -C "$TEST_REPO" commit -q -m "base"
BASE_SHA="$(git -C "$TEST_REPO" rev-parse HEAD)"

export SWARM_ISSUE_WORKER_TEST_MODE=1
export SWARM_REPO_DIR="$TEST_REPO"
export SWARM_ISSUE_WORKER_STATE_DIR="$TEST_STATE"
# shellcheck source=swarm_issue_worker.sh
source "$SCRIPT_DIR/swarm_issue_worker.sh"
mkdir -p -- "$STATE_DIR"

WORK_TYPE="followup"
PREVIOUS_AI="Claude"
CLAUDE_AVAILABLE=1
CODEX_AVAILABLE=1
select_available_ai_for_issue
test "$SELECTED_AI" = "Codex"

PREVIOUS_AI="Codex"
select_available_ai_for_issue
test "$SELECTED_AI" = "Claude"
test -n "$AI_SESSION_ID"

PREVIOUS_AI="Claude"
CODEX_AVAILABLE=0
select_available_ai_for_issue
test "$SELECTED_AI" = "Claude"

PARSED_PREVIOUS_AI="$(printf '%s' 'Completed by **Codex**.' | "$JQ_BIN" -Rr \
    'capture("(?:Completed|Reworked) by \\*\\*(?<ai>Claude|Codex)\\*\\*").ai')"
test "$PARSED_PREVIOUS_AI" = "Codex"

"$JQ_BIN" -n \
    --arg base_sha "$BASE_SHA" '
    {
        issue_number: 101,
        issue_title: "Paused work",
        issue_url: "https://example.invalid/issues/101",
        base_sha: $base_sha,
        work_type: "initial",
        previous_commit_sha: "",
        previous_completion_comment: null,
        followup_comments: [],
        trigger_comment_id: null,
        ai_tool: "Claude",
        model: "test-model",
        effort: "high",
        session_id: "session-101",
        session_comment_id: 0,
        status: "quota_paused",
        quota_pause_count: 1,
        quota_paused_at: "2026-08-25T10:00:00-0500",
        attempt_start_sha: $base_sha
    }
' > "$IN_PROGRESS_FILE"

printf 'paused change\n' >> "$TEST_REPO/tracked.txt"
printf 'untracked change\n' > "$TEST_REPO/untracked.txt"
suspend_quota_paused_issue

PAUSED_FILE="$PAUSED_ISSUES_DIR/101.json"
test -f "$PAUSED_FILE"
test ! -e "$IN_PROGRESS_FILE"
test -z "$(git -C "$TEST_REPO" status --porcelain)"
STASH_OID="$("$JQ_BIN" -r '.worktree_stash_oid' "$PAUSED_FILE")"
git -C "$TEST_REPO" cat-file -e "$STASH_OID^{commit}"

# Simulate a different issue completing while #101 remains paused. Restoring
# #101 must preserve this newer main commit and reapply only its shelved work.
printf 'other issue\n' > "$TEST_REPO/other-issue.txt"
git -C "$TEST_REPO" add other-issue.txt
git -C "$TEST_REPO" commit -q -m "other issue"
OTHER_ISSUE_SHA="$(git -C "$TEST_REPO" rev-parse HEAD)"

restore_quota_paused_issue "$PAUSED_FILE"

test -f "$IN_PROGRESS_FILE"
test ! -e "$PAUSED_FILE"
test "$("$JQ_BIN" -r '.status' "$IN_PROGRESS_FILE")" = "active"
test "$("$JQ_BIN" -r '.worktree_stash_oid // empty' "$IN_PROGRESS_FILE")" = ""
test "$(git -C "$TEST_REPO" rev-parse HEAD)" = "$OTHER_ISSUE_SHA"
grep -Fqx 'paused change' "$TEST_REPO/tracked.txt"
grep -Fqx 'untracked change' "$TEST_REPO/untracked.txt"

# The paused issue number set is what keeps selection from starting a second
# session for an already-shelved issue.
"$JQ_BIN" '.status = "quota_paused"' "$IN_PROGRESS_FILE" > "$TEST_STATE/repaused.json"
mv -- "$TEST_STATE/repaused.json" "$IN_PROGRESS_FILE"
save_attempt_start_sha "$OTHER_ISSUE_SHA"
suspend_quota_paused_issue
load_paused_issue_numbers
test "$(printf '%s' "$PAUSED_ISSUES_JSON" | "$JQ_BIN" -r 'index(101) != null')" = "true"

# Legacy paused state did not record attempt_start_sha. If main advanced after
# the pause, migration must select the commit that existed at quota_paused_at,
# not incorrectly claim the newer unrelated HEAD for the paused issue.
LEGACY_REPO="$TEST_DIR/legacy-repo"
LEGACY_STATE="$TEST_DIR/legacy-state"
mkdir -p -- "$LEGACY_REPO" "$LEGACY_STATE"
git -C "$LEGACY_REPO" init -q -b main
git -C "$LEGACY_REPO" config user.name "SWARM worker test"
git -C "$LEGACY_REPO" config user.email "worker-test@example.invalid"
printf 'base\n' > "$LEGACY_REPO/file.txt"
git -C "$LEGACY_REPO" add file.txt
GIT_AUTHOR_DATE='2026-08-25T09:00:00-0500' \
GIT_COMMITTER_DATE='2026-08-25T09:00:00-0500' \
    git -C "$LEGACY_REPO" commit -q -m "base"
LEGACY_BASE_SHA="$(git -C "$LEGACY_REPO" rev-parse HEAD)"
printf 'issue work\n' >> "$LEGACY_REPO/file.txt"
git -C "$LEGACY_REPO" add file.txt
GIT_AUTHOR_DATE='2026-08-25T09:30:00-0500' \
GIT_COMMITTER_DATE='2026-08-25T09:30:00-0500' \
    git -C "$LEGACY_REPO" commit -q -m "partial issue #202"
LEGACY_CANDIDATE_SHA="$(git -C "$LEGACY_REPO" rev-parse HEAD)"
LEGACY_DAMAGED_CANDIDATE_SHA="${LEGACY_CANDIDATE_SHA:0:8}00000000000000000000000000000000"
printf 'later\n' > "$LEGACY_REPO/later.txt"
git -C "$LEGACY_REPO" add later.txt
GIT_AUTHOR_DATE='2026-08-25T10:30:00-0500' \
GIT_COMMITTER_DATE='2026-08-25T10:30:00-0500' \
    git -C "$LEGACY_REPO" commit -q -m "unrelated later work"

REPO_DIR="$LEGACY_REPO"
STATE_DIR="$LEGACY_STATE"
IN_PROGRESS_FILE="$STATE_DIR/in-progress-issue.json"
PAUSED_ISSUES_DIR="$STATE_DIR/quota-paused-issues"
"$JQ_BIN" -n \
    --arg base_sha "$LEGACY_BASE_SHA" \
    --arg candidate_sha "$LEGACY_DAMAGED_CANDIDATE_SHA" '
    {
        issue_number: 202,
        issue_title: "Legacy paused work",
        issue_url: "https://example.invalid/issues/202",
        base_sha: $base_sha,
        candidate_sha: $candidate_sha,
        ai_tool: "Codex",
        model: "test-model",
        effort: "high",
        session_id: "session-202",
        status: "quota_paused",
        quota_paused_at: "2026-08-25T10:00:00-0500"
    }
' > "$IN_PROGRESS_FILE"
suspend_quota_paused_issue
test "$("$JQ_BIN" -r '.candidate_sha' "$PAUSED_ISSUES_DIR/202.json")" = "$LEGACY_CANDIDATE_SHA"

# A completed issue commit and unrelated dirty work can coexist when another
# process edits the shared repository during an AI run. Recovery must retain
# both facts instead of repeatedly demanding another issue commit.
RECOVERY_REPO="$TEST_DIR/recovery-repo"
RECOVERY_STATE="$TEST_DIR/recovery-state"
mkdir -p -- "$RECOVERY_REPO" "$RECOVERY_STATE"
git -C "$RECOVERY_REPO" init -q -b main
git -C "$RECOVERY_REPO" config user.name "SWARM worker test"
git -C "$RECOVERY_REPO" config user.email "worker-test@example.invalid"
printf 'base\n' > "$RECOVERY_REPO/issue.txt"
git -C "$RECOVERY_REPO" add issue.txt
git -C "$RECOVERY_REPO" commit -q -m "base"
BASE_SHA="$(git -C "$RECOVERY_REPO" rev-parse HEAD)"
printf 'fixed\n' >> "$RECOVERY_REPO/issue.txt"
git -C "$RECOVERY_REPO" add issue.txt
git -C "$RECOVERY_REPO" commit -q -m "complete issue #303"
RUN_START_SHA="$(git -C "$RECOVERY_REPO" rev-parse HEAD)"
printf 'unrelated\n' > "$RECOVERY_REPO/unrelated.txt"
REPO_DIR="$RECOVERY_REPO"
STATE_DIR="$RECOVERY_STATE"
IN_PROGRESS_FILE="$STATE_DIR/in-progress-issue.json"
ISSUE_NUMBER=303
RECOVERY_CANDIDATE_SHA=""
printf '{"issue_number":303}\n' > "$IN_PROGRESS_FILE"
prepare_recovery_repository_state
test "$RECOVERY_CANDIDATE_SHA" = "$RUN_START_SHA"
test "$("$JQ_BIN" -r '.candidate_sha' "$IN_PROGRESS_FILE")" = "$RUN_START_SHA"
test "$RECOVERY_HAS_DIRTY_WORKTREE" -eq 1
preserve_dirty_worktree_after_completion
test -f "$RECOVERY_REPO/unrelated.txt"

printf 'swarm issue worker state tests passed\n'
