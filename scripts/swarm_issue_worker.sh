#!/usr/bin/env bash

# Processes at most one assigned GitHub issue per invocation. The foreground
# runner invokes this every ten minutes; the atomic lock below makes overlapping
# runs exit immediately. Successfully handled issue numbers are kept outside the
# repository so an issue that remains open is not implemented repeatedly.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="${SWARM_REPO_DIR:-$(cd -- "$SCRIPT_DIR/.." && pwd)}"
USER_HOME_DIR="${HOME:?HOME must be set}"
STATE_DIR="${SWARM_ISSUE_WORKER_STATE_DIR:-$USER_HOME_DIR/.local/state/swarm-issue-worker}"
LOCK_DIR="$STATE_DIR/worker.lock"
COMPLETED_ISSUES_FILE="$STATE_DIR/completed-issues"
PENDING_EMAIL_FILE="$STATE_DIR/pending-email.json"
AI_OUTPUT_FILE="$STATE_DIR/last-ai-output.log"
AI_DIAGNOSTIC_FILE="$STATE_DIR/last-ai-diagnostic.log"
GITHUB_COMMENT_FILE=""
PENDING_STATE_TEMP=""
COMMIT_MESSAGE_FILE=""
UPDATED_COMMIT_MESSAGE_FILE=""

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

# cron starts with a deliberately small PATH on macOS.
export PATH="$USER_HOME_DIR/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"

GH_BIN="${GH_BIN:-$(command -v gh || true)}"
JQ_BIN="${JQ_BIN:-$(command -v jq || true)}"
CURL_BIN="${CURL_BIN:-$(command -v curl || true)}"
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

claude_has_capacity() {
    local credentials_json=""
    local access_token=""
    local usage_file
    local summary

    if [ -f "$USER_HOME_DIR/.claude/.credentials.json" ]; then
        credentials_json="$(<"$USER_HOME_DIR/.claude/.credentials.json")"
    elif [ -x /usr/bin/security ]; then
        credentials_json="$(/usr/bin/security find-generic-password -s 'Claude Code-credentials' -w 2>/dev/null || true)"
    fi

    if [ -n "$credentials_json" ]; then
        access_token="$(printf '%s' "$credentials_json" | "$JQ_BIN" -r '.claudeAiOauth.accessToken // .accessToken // empty' 2>/dev/null || true)"
    fi
    unset credentials_json

    if [ -z "$access_token" ]; then
        log "Claude quota unavailable: no Claude Code OAuth token could be read."
        return 1
    fi

    usage_file="$(mktemp "$STATE_DIR/claude-usage.XXXXXX")"
    # Feed the bearer header over stdin so the OAuth token never appears in
    # curl's process arguments.
    if ! printf '%s\n' \
        "header = \"Authorization: Bearer $access_token\"" \
        'header = "anthropic-beta: oauth-2025-04-20"' \
        'header = "Content-Type: application/json"' \
        | "$CURL_BIN" -fsS --max-time 20 --config - \
            'https://api.anthropic.com/api/oauth/usage' > "$usage_file"; then
        unset access_token
        rm -f -- "$usage_file"
        log "Claude quota unavailable: the usage request failed."
        return 1
    fi
    unset access_token

    summary="$("$JQ_BIN" -r '
        def remaining($window):
            if ($window | type) == "object"
               and ($window.utilization | type) == "number"
            then "\(100 - $window.utilization)%"
            else "unavailable"
            end;
        "5-hour: \(remaining(.five_hour)); 7-day: \(remaining(.seven_day))"
    ' "$usage_file" 2>/dev/null || printf 'unparseable response')"
    log "Claude remaining quota — $summary."

    if "$JQ_BIN" -e --arg minimum "$MIN_REMAINING_PERCENT" '
        [.five_hour, .seven_day] as $windows
        | all($windows[];
            type == "object"
            and (.utilization | type == "number")
            and .utilization <= (100 - ($minimum | tonumber)))
    ' "$usage_file" >/dev/null 2>&1; then
        rm -f -- "$usage_file"
        return 0
    fi

    rm -f -- "$usage_file"
    return 1
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
require_tool "$CURL_BIN" curl
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
    log "Neither Claude nor Codex has at least $MIN_REMAINING_PERCENT% in every active quota window; stopping."
    exit 0
fi
log "Selected $SELECTED_AI model $SELECTED_MODEL with effort $SELECTED_EFFORT for this run."

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

ISSUE_JSON="$("$JQ_BIN" -c \
    --arg assignee "$GITHUB_ASSIGNEE" \
    --argjson completed "$COMPLETED_JSON" '
        (add // [])
        | map(select(.pull_request == null))
        | map(select(any(.assignees[]?; .login == $assignee)))
        | map(select(.number as $number | ($completed | index($number)) == null))
        | sort_by(.created_at)
        | .[0] // empty
    ' "$ISSUES_FILE")"

if [ -z "$ISSUE_JSON" ]; then
    log "No unprocessed open issue assigned to $GITHUB_ASSIGNEE was found."
    exit 0
fi

ISSUE_NUMBER="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.number')"
ISSUE_TITLE="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.title')"
ISSUE_DESCRIPTION="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.body // ""')"
ISSUE_TAGS="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '[.labels[].name] | join(", ")')"
ISSUE_URL="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.html_url')"
log "Selected oldest unprocessed assigned issue: #$ISSUE_NUMBER $ISSUE_TITLE"

if [ "$DRY_RUN" = "1" ]; then
    log "Dry run complete: would run $SELECTED_AI for $ISSUE_URL."
    exit 0
fi

if ! git -C "$REPO_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    fail "SWARM_REPO_DIR is not a Git repository: $REPO_DIR"
fi
if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
    fail "The repository has uncommitted changes; refusing to let an unattended agent mix them into its commit."
fi
if [ "$(git -C "$REPO_DIR" branch --show-current)" != "main" ]; then
    if ! git -C "$REPO_DIR" switch main; then
        fail "Could not switch the clean repository to main."
    fi
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
    "Implement this issue in $REPO_DIR. Follow the repository instructions, run relevant tests, and commit the completed work to main as one commit. Include #$ISSUE_NUMBER in the commit message. Do not push." \
    "Your final response is shown in the terminal and posted to GitHub. Keep it concise: summarize the problem, the solution, and verification in one to three short paragraphs or a brief list. Do not include code snippets, diffs, file contents, command transcripts, or step-by-step implementation output." \
    > "$PROMPT_FILE"

BEFORE_SHA="$(git -C "$REPO_DIR" rev-parse HEAD)"
: > "$AI_OUTPUT_FILE"

if [ "$SELECTED_AI" = "Claude" ]; then
    if ! (
        cd -- "$REPO_DIR"
        "$CLAUDE_BIN" \
            --model "$CLAUDE_MODEL" \
            --effort "$SELECTED_EFFORT" \
            --permission-mode auto \
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
if [ "$AFTER_SHA" = "$BEFORE_SHA" ]; then
    fail "$SELECTED_AI finished without creating the required commit."
fi
if [ "$(git -C "$REPO_DIR" branch --show-current)" != "main" ]; then
    fail "$SELECTED_AI changed branches; the new commit was not left on main."
fi
if ! git -C "$REPO_DIR" merge-base --is-ancestor "$BEFORE_SHA" "$AFTER_SHA"; then
    fail "$SELECTED_AI rewrote main instead of adding a descendant commit."
fi
if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
    fail "$SELECTED_AI left uncommitted changes after committing. Review $REPO_DIR manually."
fi

if ! git -C "$REPO_DIR" log -1 --format=%B "$AFTER_SHA" | grep -Eq "(^|[^0-9])#$ISSUE_NUMBER([^0-9]|$)"; then
    COMMIT_MESSAGE_FILE="$(mktemp "$STATE_DIR/commit-message.XXXXXX")"
    UPDATED_COMMIT_MESSAGE_FILE="$(mktemp "$STATE_DIR/updated-commit-message.XXXXXX")"
    git -C "$REPO_DIR" log -1 --format=%B "$AFTER_SHA" > "$COMMIT_MESSAGE_FILE"
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
    log "Added issue #$ISSUE_NUMBER to the commit message."
fi

COMMIT_MESSAGE="$(git -C "$REPO_DIR" log -1 --format=%s "$AFTER_SHA")"
PENDING_EMAIL_TEMP="$(mktemp "$STATE_DIR/pending-email.XXXXXX")"
"$JQ_BIN" -n \
    --argjson issue_number "$ISSUE_NUMBER" \
    --arg issue_title "$ISSUE_TITLE" \
    --arg issue_url "$ISSUE_URL" \
    --arg ai "$SELECTED_AI" \
    --arg model "$SELECTED_MODEL" \
    --arg effort "$SELECTED_EFFORT" \
    --rawfile ai_output "$AI_OUTPUT_FILE" \
    --arg commit_sha "$AFTER_SHA" \
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
log "Finished issue #$ISSUE_NUMBER with $SELECTED_AI: $COMMIT_MESSAGE ($AFTER_SHA)."
