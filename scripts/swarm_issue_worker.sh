#!/usr/bin/env bash

# Processes at most one assigned GitHub issue per invocation. The foreground
# runner invokes this every ten minutes; the atomic lock below makes overlapping
# runs exit immediately. Successfully handled issue numbers are kept outside the
# repository so an issue that remains open is not implemented repeatedly.

set -euo pipefail

SCRIPT_DIR="${SWARM_ISSUE_WORKER_SCRIPT_DIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)}"
REPO_DIR="${SWARM_REPO_DIR:-$(cd -- "$SCRIPT_DIR/.." && pwd)}"
USER_HOME_DIR="${HOME:?HOME must be set}"
STATE_DIR="${SWARM_ISSUE_WORKER_STATE_DIR:-$USER_HOME_DIR/.local/state/swarm-issue-worker}"
LOCK_DIR="$STATE_DIR/worker.lock"
COMPLETED_ISSUES_FILE="$STATE_DIR/completed-issues"
PENDING_EMAIL_FILE="$STATE_DIR/pending-email.json"
IN_PROGRESS_FILE="$STATE_DIR/in-progress-issue.json"
AI_OUTPUT_FILE="$STATE_DIR/last-ai-output.log"
AI_DIAGNOSTIC_FILE="$STATE_DIR/last-ai-diagnostic.log"
GITHUB_COMMENT_FILE=""
PENDING_STATE_TEMP=""
COMMIT_MESSAGE_FILE=""
UPDATED_COMMIT_MESSAGE_FILE=""
AI_OUTPUT_TEMP=""
IN_PROGRESS_TEMP=""

GITHUB_REPOSITORY="${SWARM_GITHUB_REPOSITORY:-DotNetRockStar/swarm}"
GITHUB_ASSIGNEE="${SWARM_GITHUB_ASSIGNEE:-DotNetRockStar}"
READY_FOR_TESTING_LABEL="${SWARM_READY_FOR_TESTING_LABEL:-Ready For Testing}"
MIN_REMAINING_PERCENT="${SWARM_MIN_REMAINING_PERCENT:-5}"
CLAUDE_MODEL="${SWARM_CLAUDE_MODEL:-claude-sonnet-5}"
CODEX_MODEL="${SWARM_CODEX_MODEL:-gpt-5.6-sol}"
CLAUDE_EFFORT="${SWARM_CLAUDE_EFFORT:-high}"
CODEX_EFFORT="${SWARM_CODEX_EFFORT:-high}"
EMAIL_TO="${SWARM_EMAIL_TO:-mr_jerrodh@hotmail.com}"
SMTP_CREDENTIALS_FILE="${SWARM_SMTP_CREDENTIALS_FILE:-}"
SMTP_PASSWORD_INPUT="${SWARM_SMTP_PASSWORD:-}"
unset SWARM_SMTP_PASSWORD
DRY_RUN="${SWARM_ISSUE_WORKER_DRY_RUN:-0}"
ISSUE_COMPLETED_EXIT_CODE=10

# cron starts with a deliberately small PATH on macOS.
export PATH="$USER_HOME_DIR/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"

GH_BIN="${GH_BIN:-$(command -v gh || true)}"
JQ_BIN="${JQ_BIN:-$(command -v jq || true)}"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
CLAUDE_BIN="${CLAUDE_BIN:-$(command -v claude || true)}"
CODEX_BIN="${CODEX_BIN:-$(command -v codex || true)}"

ISSUES_FILE=""
PROMPT_FILE=""
PENDING_EMAIL_TEMP=""

log() {
    printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S%z')" "$*" >&2
}

fail() {
    log "ERROR: $*"
    exit 1
}

cleanup() {
    if [ -n "$ISSUES_FILE" ]; then rm -f -- "$ISSUES_FILE"; fi
    if [ -n "$PROMPT_FILE" ]; then rm -f -- "$PROMPT_FILE"; fi
    if [ -n "$PENDING_EMAIL_TEMP" ]; then rm -f -- "$PENDING_EMAIL_TEMP"; fi
    if [ -n "$GITHUB_COMMENT_FILE" ]; then rm -f -- "$GITHUB_COMMENT_FILE"; fi
    if [ -n "$PENDING_STATE_TEMP" ]; then rm -f -- "$PENDING_STATE_TEMP"; fi
    if [ -n "$COMMIT_MESSAGE_FILE" ]; then rm -f -- "$COMMIT_MESSAGE_FILE"; fi
    if [ -n "$UPDATED_COMMIT_MESSAGE_FILE" ]; then rm -f -- "$UPDATED_COMMIT_MESSAGE_FILE"; fi
    if [ -n "$AI_OUTPUT_TEMP" ]; then rm -f -- "$AI_OUTPUT_TEMP"; fi
    if [ -n "$IN_PROGRESS_TEMP" ]; then rm -f -- "$IN_PROGRESS_TEMP"; fi

    if [ -f "$LOCK_DIR/pid" ] && [ "$(sed -n '1p' "$LOCK_DIR/pid" 2>/dev/null || true)" = "$$" ]; then
        rm -f -- "$LOCK_DIR/pid"
        rmdir -- "$LOCK_DIR" 2>/dev/null || true
    fi
}

acquire_lock() {
    local owner_pid

    mkdir -p -- "$STATE_DIR"
    if mkdir -- "$LOCK_DIR" 2>/dev/null; then
        printf '%s\n' "$$" > "$LOCK_DIR/pid"
        return 0
    fi

    owner_pid="$(sed -n '1p' "$LOCK_DIR/pid" 2>/dev/null || true)"
    if [[ "$owner_pid" =~ ^[0-9]+$ ]] && kill -0 "$owner_pid" 2>/dev/null; then
        log "Another worker is already running as pid $owner_pid; skipping this run."
        exit 0
    fi

    # Recover an abandoned lock after a crash or forced reboot.
    rm -f -- "$LOCK_DIR/pid" 2>/dev/null || true
    rmdir -- "$LOCK_DIR" 2>/dev/null || true
    if ! mkdir -- "$LOCK_DIR" 2>/dev/null; then
        log "Another worker acquired the lock while stale-lock recovery was running; skipping."
        exit 0
    fi
    printf '%s\n' "$$" > "$LOCK_DIR/pid"
}

require_tool() {
    local path="$1"
    local name="$2"
    if [ -z "$path" ] || [ ! -x "$path" ]; then
        fail "$name is required but was not found in PATH."
    fi
}

clear_in_progress_issue() {
    local issue_number="$1"
    local saved_issue_number

    [ -f "$IN_PROGRESS_FILE" ] || return 0
    saved_issue_number="$("$JQ_BIN" -r '.issue_number // empty' "$IN_PROGRESS_FILE" 2>/dev/null || true)"
    if [ "$saved_issue_number" = "$issue_number" ]; then
        rm -f -- "$IN_PROGRESS_FILE"
    fi
}

save_in_progress_issue() {
    local issue_number="$1"
    local issue_title="$2"
    local issue_url="$3"
    local base_sha="$4"

    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" -n \
        --argjson issue_number "$issue_number" \
        --arg issue_title "$issue_title" \
        --arg issue_url "$issue_url" \
        --arg base_sha "$base_sha" \
        --arg started_at "$(date '+%Y-%m-%dT%H:%M:%S%z')" '
            {
                issue_number: $issue_number,
                issue_title: $issue_title,
                issue_url: $issue_url,
                base_sha: $base_sha,
                started_at: $started_at
            }
        ' > "$IN_PROGRESS_TEMP"
    mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
    IN_PROGRESS_TEMP=""
}

claude_has_capacity() {
    local usage_file
    local usage_result
    local session_used
    local week_used
    local session_remaining
    local week_remaining

    usage_file="$(mktemp "$STATE_DIR/claude-usage.XXXXXX")"
    if ! "$CLAUDE_BIN" -p '/usage' \
        --output-format json \
        --tools '' \
        --no-session-persistence > "$usage_file" 2>&1; then
        rm -f -- "$usage_file"
        log "Claude quota unavailable: Claude Code's /usage command failed. Run 'claude auth login' if this persists."
        return 1
    fi

    usage_result="$("$JQ_BIN" -r '.result // empty' "$usage_file" 2>/dev/null || true)"
    rm -f -- "$usage_file"
    session_used="$(printf '%s\n' "$usage_result" \
        | sed -nE 's/^Current session:[[:space:]]*([0-9]+([.][0-9]+)?)% used.*$/\1/p' \
        | sed -n '1p')"
    week_used="$(printf '%s\n' "$usage_result" \
        | sed -nE 's/^Current week( \([^)]*\))?:[[:space:]]*([0-9]+([.][0-9]+)?)% used.*$/\2/p' \
        | sed -n '1p')"

    if [ -z "$session_used" ] || [ -z "$week_used" ]; then
        log "Claude quota unavailable: Claude Code returned an unrecognized /usage format."
        return 1
    fi

    session_remaining="$("$JQ_BIN" -nr --arg used "$session_used" '100 - ($used | tonumber)')"
    week_remaining="$("$JQ_BIN" -nr --arg used "$week_used" '100 - ($used | tonumber)')"
    log "Claude remaining quota — session: $session_remaining%; week: $week_remaining%."

    "$JQ_BIN" -en \
        --arg session "$session_remaining" \
        --arg week "$week_remaining" \
        --arg minimum "$MIN_REMAINING_PERCENT" '
            ($session | tonumber) >= ($minimum | tonumber)
            and ($week | tonumber) >= ($minimum | tonumber)
        ' >/dev/null
}

codex_has_capacity() {
    local usage_file
    local summary

    if [ -z "$CODEX_BIN" ] || [ ! -x "$CODEX_BIN" ]; then
        log "Codex quota unavailable: codex was not found in PATH."
        return 1
    fi

    usage_file="$(mktemp "$STATE_DIR/codex-usage.XXXXXX")"
    if ! "$PYTHON_BIN" "$SCRIPT_DIR/codex_rate_limits.py" \
        --codex-bin "$CODEX_BIN" > "$usage_file"; then
        rm -f -- "$usage_file"
        log "Codex quota unavailable: the local rate-limit request failed."
        return 1
    fi

    summary="$("$JQ_BIN" -r '
        def remaining($window):
            if ($window | type) == "object"
               and ($window.usedPercent | type) == "number"
            then "\(100 - $window.usedPercent)%"
            else "unavailable"
            end;
        "primary: \(remaining(.primary)); secondary: \(remaining(.secondary))"
    ' "$usage_file" 2>/dev/null || printf 'unparseable response')"
    log "Codex remaining quota — $summary."

    if "$JQ_BIN" -e --arg minimum "$MIN_REMAINING_PERCENT" '
        . as $limits
        | ([.primary, .secondary] | map(select(. != null))) as $windows
        | ($limits.rateLimitReachedType == null)
          and (($limits.spendControlReached // false) == false)
          and ($windows | length > 0)
          and all($windows[];
              (.usedPercent | type == "number")
              and .usedPercent <= (100 - ($minimum | tonumber)))
    ' "$usage_file" >/dev/null 2>&1; then
        rm -f -- "$usage_file"
        return 0
    fi

    rm -f -- "$usage_file"
    return 1
}

deliver_pending_email() {
    local issue_number

    [ -f "$PENDING_EMAIL_FILE" ] || return 0
    [ -n "$SMTP_CREDENTIALS_FILE" ] || fail "Set SWARM_SMTP_CREDENTIALS_FILE to a settings file that does not contain the SMTP password."
    [ -f "$SMTP_CREDENTIALS_FILE" ] || fail "SMTP settings file was not found: $SMTP_CREDENTIALS_FILE"
    [ -n "$SMTP_PASSWORD_INPUT" ] || fail "The SMTP password must be entered through the foreground runner."
    issue_number="$("$JQ_BIN" -r '.issue_number' "$PENDING_EMAIL_FILE")"
    log "Sending notification for issue #$issue_number."

    printf '%s\n' "$SMTP_PASSWORD_INPUT" | "$PYTHON_BIN" "$SCRIPT_DIR/send_issue_notification.py" \
        --credentials "$SMTP_CREDENTIALS_FILE" \
        --password-stdin \
        --to "$EMAIL_TO" \
        --issue-number "$issue_number" \
        --issue-title "$("$JQ_BIN" -r '.issue_title' "$PENDING_EMAIL_FILE")" \
        --issue-url "$("$JQ_BIN" -r '.issue_url' "$PENDING_EMAIL_FILE")" \
        --ai "$("$JQ_BIN" -r '.ai' "$PENDING_EMAIL_FILE")" \
        --commit-sha "$("$JQ_BIN" -r '.commit_sha' "$PENDING_EMAIL_FILE")" \
        --commit-message "$("$JQ_BIN" -r '.commit_message' "$PENDING_EMAIL_FILE")"

    if ! grep -Fxq -- "$issue_number" "$COMPLETED_ISSUES_FILE" 2>/dev/null; then
        printf '%s\n' "$issue_number" >> "$COMPLETED_ISSUES_FILE"
    fi
    rm -f -- "$PENDING_EMAIL_FILE"
    clear_in_progress_issue "$issue_number"
    log "Notification sent; issue #$issue_number is marked completed locally."
}

post_pending_github_comment() {
    local issue_number
    local commit_sha
    local marker
    local already_posted

    if "$JQ_BIN" -e '.github_comment_posted // false' "$PENDING_EMAIL_FILE" >/dev/null; then
        return 0
    fi

    issue_number="$("$JQ_BIN" -r '.issue_number' "$PENDING_EMAIL_FILE")"
    commit_sha="$("$JQ_BIN" -r '.commit_sha' "$PENDING_EMAIL_FILE")"
    marker="<!-- swarm-issue-worker:commit:$commit_sha -->"
    already_posted="$(
        "$GH_BIN" api --method GET --paginate --slurp \
            "repos/$GITHUB_REPOSITORY/issues/$issue_number/comments" \
            -F per_page=100 \
        | "$JQ_BIN" -r --arg marker "$marker" \
            '[add // [] | .[] | select((.body // "") | contains($marker))] | length'
    )"

    if [ "$already_posted" -eq 0 ]; then
        GITHUB_COMMENT_FILE="$(mktemp "$STATE_DIR/github-comment.XXXXXX")"
        {
            printf '%s\n' "$marker"
            printf 'Completed by **%s**.\n\n' "$("$JQ_BIN" -r '.ai_tool // .ai' "$PENDING_EMAIL_FILE")"
            printf -- '- Model: `%s`\n' "$("$JQ_BIN" -r '.model // "unknown"' "$PENDING_EMAIL_FILE")"
            printf -- '- Effort: `%s`\n' "$("$JQ_BIN" -r '.effort // "unknown"' "$PENDING_EMAIL_FILE")"
            printf -- '- Commit: `%s` — %s\n\n' \
                "$commit_sha" \
                "$("$JQ_BIN" -r '.commit_message' "$PENDING_EMAIL_FILE")"
            printf '<details><summary>AI completion summary</summary>\n\n'
            "$JQ_BIN" -r '.ai_output // "(No captured AI output was available.)"' "$PENDING_EMAIL_FILE" \
                | sed 's/^/    /'
            printf '\n</details>\n'
        } > "$GITHUB_COMMENT_FILE"

        log "Posting the AI completion summary to GitHub issue #$issue_number."
        "$GH_BIN" issue comment "$issue_number" \
            --repo "$GITHUB_REPOSITORY" \
            --body-file "$GITHUB_COMMENT_FILE"
        rm -f -- "$GITHUB_COMMENT_FILE"
        GITHUB_COMMENT_FILE=""
    else
        log "GitHub issue #$issue_number already has the completion response for $commit_sha."
    fi

    PENDING_STATE_TEMP="$(mktemp "$STATE_DIR/pending-state.XXXXXX")"
    "$JQ_BIN" '.github_comment_posted = true' "$PENDING_EMAIL_FILE" > "$PENDING_STATE_TEMP"
    mv -- "$PENDING_STATE_TEMP" "$PENDING_EMAIL_FILE"
    PENDING_STATE_TEMP=""
}

add_pending_ready_for_testing_label() {
    local issue_number

    if "$JQ_BIN" -e '.ready_for_testing_label_added // false' "$PENDING_EMAIL_FILE" >/dev/null; then
        return 0
    fi

    issue_number="$("$JQ_BIN" -r '.issue_number' "$PENDING_EMAIL_FILE")"
    log "Adding the '$READY_FOR_TESTING_LABEL' label to GitHub issue #$issue_number."
    "$GH_BIN" issue edit "$issue_number" \
        --repo "$GITHUB_REPOSITORY" \
        --add-label "$READY_FOR_TESTING_LABEL"

    PENDING_STATE_TEMP="$(mktemp "$STATE_DIR/pending-state.XXXXXX")"
    "$JQ_BIN" '.ready_for_testing_label_added = true' "$PENDING_EMAIL_FILE" > "$PENDING_STATE_TEMP"
    mv -- "$PENDING_STATE_TEMP" "$PENDING_EMAIL_FILE"
    PENDING_STATE_TEMP=""
}

trap cleanup EXIT
trap 'exit 130' INT TERM

acquire_lock
require_tool "$GH_BIN" gh
require_tool "$JQ_BIN" jq
require_tool "$PYTHON_BIN" python3

if [ -f "$PENDING_EMAIL_FILE" ]; then
    if [ "$DRY_RUN" = "1" ]; then
        log "Dry run: a pending notification exists; no email or AI work was performed."
        exit 0
    fi
    post_pending_github_comment
    add_pending_ready_for_testing_label
    deliver_pending_email
fi

ISSUES_FILE="$(mktemp "$STATE_DIR/github-issues.XXXXXX")"
if ! "$GH_BIN" api --method GET --paginate --slurp \
    "repos/$GITHUB_REPOSITORY/issues" \
    -f state=open \
    -f assignee="$GITHUB_ASSIGNEE" \
    -f sort=created \
    -f direction=asc \
    -F per_page=100 > "$ISSUES_FILE"; then
    fail "GitHub issue query failed. Check 'gh auth status' and network access."
fi

touch -- "$COMPLETED_ISSUES_FILE"
COMPLETED_JSON="$("$JQ_BIN" -Rsc '
    split("\n") | map(select(test("^[0-9]+$")) | tonumber)
' "$COMPLETED_ISSUES_FILE")"

IN_PROGRESS_ISSUE_NUMBER_JSON="null"
if [ -f "$IN_PROGRESS_FILE" ]; then
    if ! "$JQ_BIN" -e '
        (.issue_number | type) == "number"
        and (.base_sha | type) == "string"
        and (.base_sha | test("^[0-9a-f]{40}$"))
    ' "$IN_PROGRESS_FILE" >/dev/null 2>&1; then
        fail "The saved in-progress issue state is invalid: $IN_PROGRESS_FILE"
    fi
    IN_PROGRESS_ISSUE_NUMBER_JSON="$("$JQ_BIN" -r '.issue_number' "$IN_PROGRESS_FILE")"
    if printf '%s' "$COMPLETED_JSON" | "$JQ_BIN" -e \
        --argjson issue_number "$IN_PROGRESS_ISSUE_NUMBER_JSON" \
        'index($issue_number) != null' >/dev/null; then
        clear_in_progress_issue "$IN_PROGRESS_ISSUE_NUMBER_JSON"
        IN_PROGRESS_ISSUE_NUMBER_JSON="null"
    fi
fi

ISSUE_JSON="$("$JQ_BIN" -c \
    --arg assignee "$GITHUB_ASSIGNEE" \
    --argjson completed "$COMPLETED_JSON" \
    --argjson in_progress "$IN_PROGRESS_ISSUE_NUMBER_JSON" '
        (add // [])
        | map(select(.pull_request == null))
        | map(select(any(.assignees[]?; .login == $assignee)))
        | map(select(.number as $number | ($completed | index($number)) == null))
        | sort_by(.created_at)
        | if $in_progress == null
          then .[0] // empty
          else map(select(.number == $in_progress)) | .[0] // empty
          end
    ' "$ISSUES_FILE")"

if [ -z "$ISSUE_JSON" ]; then
    if [ "$IN_PROGRESS_ISSUE_NUMBER_JSON" != "null" ]; then
        fail "Saved in-progress issue #$IN_PROGRESS_ISSUE_NUMBER_JSON is no longer open and assigned to $GITHUB_ASSIGNEE; review $IN_PROGRESS_FILE."
    fi
    log "No unprocessed open issue assigned to $GITHUB_ASSIGNEE was found."
    exit 0
fi

ISSUE_NUMBER="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.number')"
ISSUE_TITLE="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.title')"
ISSUE_DESCRIPTION="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.body // ""')"
ISSUE_TAGS="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '[.labels[].name] | join(", ")')"
ISSUE_URL="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.html_url')"
log "Selected oldest unprocessed assigned issue: #$ISSUE_NUMBER $ISSUE_TITLE"

SELECTED_AI=""
SELECTED_MODEL=""
SELECTED_EFFORT=""
CLAUDE_AVAILABLE=0
CODEX_AVAILABLE=0

if [ -n "$CLAUDE_BIN" ] && [ -x "$CLAUDE_BIN" ]; then
    if claude_has_capacity; then
        CLAUDE_AVAILABLE=1
    fi
else
    log "Claude remaining quota — unavailable (claude was not found in PATH)."
fi
if codex_has_capacity; then
    CODEX_AVAILABLE=1
fi

if [ "$CLAUDE_AVAILABLE" -eq 1 ]; then
    SELECTED_AI="Claude"
    SELECTED_MODEL="$CLAUDE_MODEL"
    SELECTED_EFFORT="$CLAUDE_EFFORT"
elif [ "$CODEX_AVAILABLE" -eq 1 ]; then
    SELECTED_AI="Codex"
    SELECTED_MODEL="$CODEX_MODEL"
    SELECTED_EFFORT="$CODEX_EFFORT"
else
    log "Neither Claude nor Codex has at least $MIN_REMAINING_PERCENT% remaining in every active quota window; stopping."
    exit 0
fi
log "Selected $SELECTED_AI model $SELECTED_MODEL with effort $SELECTED_EFFORT for this run."

if [ "$DRY_RUN" = "1" ]; then
    log "Dry run complete: would run $SELECTED_AI for $ISSUE_URL."
    exit 0
fi

if ! git -C "$REPO_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    fail "SWARM_REPO_DIR is not a Git repository: $REPO_DIR"
fi
if [ "$(git -C "$REPO_DIR" branch --show-current)" != "main" ]; then
    if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
        fail "The repository has uncommitted changes on a non-main branch; refusing unattended recovery."
    fi
    if ! git -C "$REPO_DIR" switch main; then
        fail "Could not switch the clean repository to main."
    fi
fi

RUN_START_SHA="$(git -C "$REPO_DIR" rev-parse HEAD)"
BASE_SHA="$RUN_START_SHA"
RECOVERY_MODE=0
RECOVERY_CANDIDATE_SHA=""
RECOVERY_HAS_DIRTY_WORKTREE=0

if [ -f "$IN_PROGRESS_FILE" ]; then
    SAVED_ISSUE_NUMBER="$("$JQ_BIN" -r '.issue_number' "$IN_PROGRESS_FILE")"
    if [ "$SAVED_ISSUE_NUMBER" != "$ISSUE_NUMBER" ]; then
        fail "Issue #$SAVED_ISSUE_NUMBER is already in progress; refusing to start issue #$ISSUE_NUMBER."
    fi
    BASE_SHA="$("$JQ_BIN" -r '.base_sha' "$IN_PROGRESS_FILE")"
    RECOVERY_CANDIDATE_SHA="$("$JQ_BIN" -r '.candidate_sha // empty' "$IN_PROGRESS_FILE")"
    if ! git -C "$REPO_DIR" cat-file -e "$BASE_SHA^{commit}" 2>/dev/null; then
        fail "The saved base commit for issue #$ISSUE_NUMBER no longer exists: $BASE_SHA"
    fi
    if ! git -C "$REPO_DIR" merge-base --is-ancestor "$BASE_SHA" "$RUN_START_SHA"; then
        fail "Main no longer descends from issue #$ISSUE_NUMBER's saved base commit; refusing unattended recovery."
    fi
    if [ -n "$RECOVERY_CANDIDATE_SHA" ]; then
        if ! git -C "$REPO_DIR" cat-file -e "$RECOVERY_CANDIDATE_SHA^{commit}" 2>/dev/null \
            || ! git -C "$REPO_DIR" merge-base --is-ancestor "$BASE_SHA" "$RECOVERY_CANDIDATE_SHA" \
            || ! git -C "$REPO_DIR" merge-base --is-ancestor "$RECOVERY_CANDIDATE_SHA" "$RUN_START_SHA"; then
            fail "The saved recovery commit for issue #$ISSUE_NUMBER is not on main after its base commit."
        fi
    fi
    RECOVERY_MODE=1

    if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
        RECOVERY_HAS_DIRTY_WORKTREE=1
        log "Resuming the uncommitted work left by the previous issue #$ISSUE_NUMBER attempt."
    elif [ "$RUN_START_SHA" != "$BASE_SHA" ]; then
        if [ -z "$RECOVERY_CANDIDATE_SHA" ]; then
            RECOVERY_CANDIDATE_SHA="$RUN_START_SHA"
            IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
            "$JQ_BIN" --arg candidate_sha "$RECOVERY_CANDIDATE_SHA" \
                '.candidate_sha = $candidate_sha' "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"
            mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
            IN_PROGRESS_TEMP=""
        fi
        log "Verifying commit $RECOVERY_CANDIDATE_SHA as the recovered implementation for issue #$ISSUE_NUMBER."
    else
        log "Retrying issue #$ISSUE_NUMBER from its original clean base commit."
    fi
else
    if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
        fail "The repository has uncommitted changes unrelated to a saved worker attempt; refusing to mix them into an unattended commit."
    fi
    save_in_progress_issue "$ISSUE_NUMBER" "$ISSUE_TITLE" "$ISSUE_URL" "$BASE_SHA"
fi

PROMPT_FILE="$(mktemp "$STATE_DIR/issue-prompt.XXXXXX")"
printf '%s\n' \
    "Issue title:" \
    "$ISSUE_TITLE" \
    "Issue number:" \
    "#$ISSUE_NUMBER" \
    "" \
    "Issue description:" \
    "$ISSUE_DESCRIPTION" \
    "" \
    "Issue tags:" \
    "${ISSUE_TAGS:-none}" \
    "" \
    "This is an unattended autopilot run. Do not ask the user questions, request confirmation, or pause for interactive input. Resolve ambiguity from the issue and repository, make reasonable safe assumptions, and implement the approach you recommend. If several valid approaches exist, choose the best maintainable option yourself. Only report a blocker when required credentials, authority, or external information are genuinely unavailable." \
    "Implement this issue in $REPO_DIR. Follow the repository instructions, run relevant tests, and commit the completed work to main as one commit. Include #$ISSUE_NUMBER in the commit message. Do not push. Run verification commands in the foreground; do not return while tests or builds are still running." \
    "Your final response is shown in the terminal and posted to GitHub. Keep it concise: summarize the problem, the solution, and verification in one to three short paragraphs or a brief list. Do not include code snippets, diffs, file contents, command transcripts, or step-by-step implementation output." \
    > "$PROMPT_FILE"

if [ "$RECOVERY_HAS_DIRTY_WORKTREE" -eq 1 ]; then
    printf '%s\n' \
        "This is a recovery run for the same issue. A previous AI attempt left the current uncommitted changes. Inspect and preserve correct work, finish the implementation and foreground verification, then commit it. Do not discard the existing work merely because the worktree is not clean." \
        >> "$PROMPT_FILE"
elif [ "$RECOVERY_MODE" -eq 1 ] && [ -n "$RECOVERY_CANDIDATE_SHA" ]; then
    printf '%s\n' \
        "This is a recovery verification run. Commit $RECOVERY_CANDIDATE_SHA was created after the original attempt began and may already implement this issue. Verify the implementation and tests. If it is complete, do not create duplicate code or rewrite history; put SWARM_RECOVERY_COMPLETE on its own final line after your concise summary. If it is incomplete, finish it and create the required commit." \
        >> "$PROMPT_FILE"
fi

: > "$AI_OUTPUT_FILE"

if [ "$SELECTED_AI" = "Claude" ]; then
    if ! (
        cd -- "$REPO_DIR"
        "$CLAUDE_BIN" \
            --model "$CLAUDE_MODEL" \
            --effort "$SELECTED_EFFORT" \
            --permission-mode bypassPermissions \
            -p - < "$PROMPT_FILE"
    ) 2>&1 | tee "$AI_OUTPUT_FILE"; then
        fail "Claude exited unsuccessfully. Its output is in $AI_OUTPUT_FILE."
    fi
else
    # Current Codex uses these equivalents for the older --yolo and --effort
    # spellings, and '-' reads the prompt from stdin (where -p now means profile).
    log "Codex is working. Detailed implementation output is hidden; its final summary will appear when finished."
    if ! "$CODEX_BIN" exec \
        -m "$CODEX_MODEL" \
        --dangerously-bypass-approvals-and-sandbox \
        --dangerously-bypass-hook-trust \
        -c "model_reasoning_effort=\"$SELECTED_EFFORT\"" \
        -C "$REPO_DIR" \
        --output-last-message "$AI_OUTPUT_FILE" \
        - < "$PROMPT_FILE" > "$AI_DIAGNOSTIC_FILE" 2>&1; then
        fail "Codex exited unsuccessfully. Diagnostic output is in $AI_DIAGNOSTIC_FILE."
    fi
    if [ ! -s "$AI_OUTPUT_FILE" ]; then
        fail "Codex finished without writing a final summary. Diagnostic output is in $AI_DIAGNOSTIC_FILE."
    fi
    printf '\n%s\n' '--- Codex completion summary ---'
    sed -n '1,$p' "$AI_OUTPUT_FILE"
fi

AFTER_SHA="$(git -C "$REPO_DIR" rev-parse HEAD)"
COMPLETION_SHA="$AFTER_SHA"
RECOVERED_EXISTING_COMMIT=0
if [ "$AFTER_SHA" != "$RUN_START_SHA" ]; then
    : # The current invocation created the completion commit.
elif [ "$RECOVERY_MODE" -eq 1 ] \
    && [ -n "$RECOVERY_CANDIDATE_SHA" ] \
    && grep -Eq '^[[:space:]]*SWARM_RECOVERY_COMPLETE[[:space:]]*$' "$AI_OUTPUT_FILE"; then
    COMPLETION_SHA="$RECOVERY_CANDIDATE_SHA"
    RECOVERED_EXISTING_COMMIT=1
    AI_OUTPUT_TEMP="$(mktemp "$STATE_DIR/ai-output.XXXXXX")"
    sed -E '/^[[:space:]]*SWARM_RECOVERY_COMPLETE[[:space:]]*$/d' "$AI_OUTPUT_FILE" > "$AI_OUTPUT_TEMP"
    mv -- "$AI_OUTPUT_TEMP" "$AI_OUTPUT_FILE"
    AI_OUTPUT_TEMP=""
    log "Accepted commit $COMPLETION_SHA as the recovered implementation for issue #$ISSUE_NUMBER."
else
    fail "$SELECTED_AI finished without creating the required commit. Recovery state was preserved for the next attempt."
fi
if [ "$(git -C "$REPO_DIR" branch --show-current)" != "main" ]; then
    fail "$SELECTED_AI changed branches; the new commit was not left on main."
fi
if ! git -C "$REPO_DIR" merge-base --is-ancestor "$BASE_SHA" "$AFTER_SHA"; then
    fail "$SELECTED_AI rewrote main instead of adding a descendant commit."
fi
if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
    fail "$SELECTED_AI left uncommitted changes after committing. Review $REPO_DIR manually."
fi

if ! git -C "$REPO_DIR" log -1 --format=%B "$COMPLETION_SHA" | grep -Eq "(^|[^0-9])#$ISSUE_NUMBER([^0-9]|$)"; then
    if [ "$RECOVERED_EXISTING_COMMIT" -eq 1 ]; then
        log "Recovered commit $COMPLETION_SHA is already established and does not mention #$ISSUE_NUMBER; leaving its history unchanged."
    else
        COMMIT_MESSAGE_FILE="$(mktemp "$STATE_DIR/commit-message.XXXXXX")"
        UPDATED_COMMIT_MESSAGE_FILE="$(mktemp "$STATE_DIR/updated-commit-message.XXXXXX")"
        git -C "$REPO_DIR" log -1 --format=%B "$COMPLETION_SHA" > "$COMMIT_MESSAGE_FILE"
        {
            IFS= read -r commit_subject || true
            printf '%s (#%s)\n' "$commit_subject" "$ISSUE_NUMBER"
            cat
        } < "$COMMIT_MESSAGE_FILE" > "$UPDATED_COMMIT_MESSAGE_FILE"
        git -C "$REPO_DIR" commit --amend --no-verify -F "$UPDATED_COMMIT_MESSAGE_FILE"
        rm -f -- "$COMMIT_MESSAGE_FILE" "$UPDATED_COMMIT_MESSAGE_FILE"
        COMMIT_MESSAGE_FILE=""
        UPDATED_COMMIT_MESSAGE_FILE=""
        AFTER_SHA="$(git -C "$REPO_DIR" rev-parse HEAD)"
        COMPLETION_SHA="$AFTER_SHA"
        log "Added issue #$ISSUE_NUMBER to the commit message."
    fi
fi

COMMIT_MESSAGE="$(git -C "$REPO_DIR" log -1 --format=%s "$COMPLETION_SHA")"
PENDING_EMAIL_TEMP="$(mktemp "$STATE_DIR/pending-email.XXXXXX")"
"$JQ_BIN" -n \
    --argjson issue_number "$ISSUE_NUMBER" \
    --arg issue_title "$ISSUE_TITLE" \
    --arg issue_url "$ISSUE_URL" \
    --arg ai "$SELECTED_AI" \
    --arg model "$SELECTED_MODEL" \
    --arg effort "$SELECTED_EFFORT" \
    --rawfile ai_output "$AI_OUTPUT_FILE" \
    --arg commit_sha "$COMPLETION_SHA" \
    --arg commit_message "$COMMIT_MESSAGE" '
        {
            issue_number: $issue_number,
            issue_title: $issue_title,
            issue_url: $issue_url,
            ai: $ai,
            ai_tool: $ai,
            model: $model,
            effort: $effort,
            ai_output: $ai_output,
            commit_sha: $commit_sha,
            commit_message: $commit_message,
            github_comment_posted: false,
            ready_for_testing_label_added: false
        }
    ' > "$PENDING_EMAIL_TEMP"
mv -- "$PENDING_EMAIL_TEMP" "$PENDING_EMAIL_FILE"
PENDING_EMAIL_TEMP=""

post_pending_github_comment
add_pending_ready_for_testing_label
deliver_pending_email
log "Finished issue #$ISSUE_NUMBER with $SELECTED_AI: $COMMIT_MESSAGE ($COMPLETION_SHA)."
exit "$ISSUE_COMPLETED_EXIT_CODE"
