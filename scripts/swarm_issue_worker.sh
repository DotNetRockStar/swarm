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
PAUSED_ISSUES_DIR="$STATE_DIR/quota-paused-issues"
AI_OUTPUT_FILE="$STATE_DIR/last-ai-output.log"
AI_DIAGNOSTIC_FILE="$STATE_DIR/last-ai-diagnostic.log"
GITHUB_COMMENT_FILE=""
PENDING_STATE_TEMP=""
COMMIT_MESSAGE_FILE=""
UPDATED_COMMIT_MESSAGE_FILE=""
AI_OUTPUT_TEMP=""
IN_PROGRESS_TEMP=""
COMMENTS_FILE=""
QUOTA_COMMENT_FILE=""

GITHUB_REPOSITORY="${SWARM_GITHUB_REPOSITORY:-DotNetRockStar/swarm}"
GITHUB_ASSIGNEE="${SWARM_GITHUB_ASSIGNEE:-DotNetRockStar}"
TRUSTED_FOLLOWUP_AUTHOR="${SWARM_TRUSTED_FOLLOWUP_AUTHOR:-$GITHUB_ASSIGNEE}"
READY_FOR_TESTING_LABEL="${SWARM_READY_FOR_TESTING_LABEL:-Ready For Testing}"
MIN_REMAINING_PERCENT="${SWARM_MIN_REMAINING_PERCENT:-10}"
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
QUOTA_PAUSED_EXIT_CODE=11

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
SELECTED_AI=""
SELECTED_MODEL=""
SELECTED_EFFORT=""
AI_SESSION_ID=""
SESSION_IS_RESUME=0
SESSION_COMMENT_ID=0
RESUME_COMMENTS_JSON="[]"
RESUME_COMMENT_ID=0
QUOTA_RESUME_READY=0
PAUSED_ISSUES_JSON="[]"
AVAILABLE_PAUSED_FILE=""

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
    if [ -n "$COMMENTS_FILE" ]; then rm -f -- "$COMMENTS_FILE"; fi
    if [ -n "$QUOTA_COMMENT_FILE" ]; then rm -f -- "$QUOTA_COMMENT_FILE"; fi

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

    base_sha="$(git -C "$REPO_DIR" rev-parse --verify "$base_sha^{commit}" 2>/dev/null)" \
        || fail "Cannot save issue #$issue_number with an invalid base commit: $4"

    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" -n \
        --argjson issue_number "$issue_number" \
        --arg issue_title "$issue_title" \
        --arg issue_url "$issue_url" \
        --arg base_sha "$base_sha" \
        --arg work_type "$WORK_TYPE" \
        --arg previous_commit_sha "$PREVIOUS_COMMIT_SHA" \
        --arg previous_ai "$PREVIOUS_AI" \
        --arg previous_completion_comment "$PREVIOUS_COMPLETION_COMMENT_JSON" \
        --arg followup_comments "$FOLLOWUP_COMMENTS_JSON" \
        --argjson trigger_comment_id "$TRIGGER_COMMENT_ID_JSON" \
        --arg ai_tool "$SELECTED_AI" \
        --arg model "$SELECTED_MODEL" \
        --arg effort "$SELECTED_EFFORT" \
        --arg session_id "$AI_SESSION_ID" \
        --argjson session_comment_id "$SESSION_COMMENT_ID" \
        --arg started_at "$(date '+%Y-%m-%dT%H:%M:%S%z')" '
            {
                issue_number: $issue_number,
                issue_title: $issue_title,
                issue_url: $issue_url,
                base_sha: $base_sha,
                work_type: $work_type,
                previous_commit_sha: $previous_commit_sha,
                previous_ai: $previous_ai,
                previous_completion_comment: ($previous_completion_comment | fromjson),
                followup_comments: ($followup_comments | fromjson),
                trigger_comment_id: $trigger_comment_id,
                ai_tool: $ai_tool,
                model: $model,
                effort: $effort,
                session_id: $session_id,
                session_comment_id: $session_comment_id,
                status: "active",
                quota_pause_count: 0,
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
        return 2
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
        return 2
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
        return 2
    fi

    usage_file="$(mktemp "$STATE_DIR/codex-usage.XXXXXX")"
    if ! "$PYTHON_BIN" "$SCRIPT_DIR/codex_rate_limits.py" \
        --codex-bin "$CODEX_BIN" > "$usage_file"; then
        rm -f -- "$usage_file"
        log "Codex quota unavailable: the local rate-limit request failed."
        return 2
    fi

    if ! "$JQ_BIN" -e '
        type == "object"
        and ([.primary, .secondary] | map(select(. != null))) as $windows
        | ($windows | length) > 0
          and all($windows[]; (.usedPercent | type) == "number")
    ' "$usage_file" >/dev/null 2>&1; then
        rm -f -- "$usage_file"
        log "Codex quota unavailable: the local rate-limit response was invalid."
        return 2
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

selected_ai_has_capacity() {
    case "$1" in
        Claude)
            if [ -z "$CLAUDE_BIN" ] || [ ! -x "$CLAUDE_BIN" ]; then
                log "Claude quota unavailable: claude was not found in PATH."
                return 2
            fi
            claude_has_capacity
            ;;
        Codex)
            codex_has_capacity
            ;;
        *)
            fail "The saved AI provider is invalid: $1"
            ;;
    esac
}

select_available_ai_for_issue() {
    local preferred_ai=""

    if [ "$WORK_TYPE" = "followup" ]; then
        case "$PREVIOUS_AI" in
            Claude) preferred_ai="Codex" ;;
            Codex) preferred_ai="Claude" ;;
        esac
    fi

    if [ "$preferred_ai" = "Codex" ] && [ "$CODEX_AVAILABLE" -eq 1 ]; then
        SELECTED_AI="Codex"
        SELECTED_MODEL="$CODEX_MODEL"
        SELECTED_EFFORT="$CODEX_EFFORT"
        AI_SESSION_ID=""
        log "Follow-up review prefers Codex because Claude completed the previous pass."
    elif [ "$preferred_ai" = "Claude" ] && [ "$CLAUDE_AVAILABLE" -eq 1 ]; then
        SELECTED_AI="Claude"
        SELECTED_MODEL="$CLAUDE_MODEL"
        SELECTED_EFFORT="$CLAUDE_EFFORT"
        AI_SESSION_ID="$("$PYTHON_BIN" -c 'import uuid; print(uuid.uuid4())')"
        log "Follow-up review prefers Claude because Codex completed the previous pass."
    elif [ "$CLAUDE_AVAILABLE" -eq 1 ]; then
        SELECTED_AI="Claude"
        SELECTED_MODEL="$CLAUDE_MODEL"
        SELECTED_EFFORT="$CLAUDE_EFFORT"
        AI_SESSION_ID="$("$PYTHON_BIN" -c 'import uuid; print(uuid.uuid4())')"
        if [ -n "$preferred_ai" ]; then
            log "$preferred_ai lacks capacity; falling back to Claude for this follow-up."
        fi
    elif [ "$CODEX_AVAILABLE" -eq 1 ]; then
        SELECTED_AI="Codex"
        SELECTED_MODEL="$CODEX_MODEL"
        SELECTED_EFFORT="$CODEX_EFFORT"
        AI_SESSION_ID=""
        if [ -n "$preferred_ai" ]; then
            log "$preferred_ai lacks capacity; falling back to Codex for this follow-up."
        fi
    else
        return 1
    fi
}

update_in_progress() {
    local filter="$1"

    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" "$filter" "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"
    mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
    IN_PROGRESS_TEMP=""
}

save_codex_session_id() {
    local session_id="$1"

    [ -n "$session_id" ] || return 0
    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" --arg session_id "$session_id" \
        '.session_id = $session_id' "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"
    mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
    IN_PROGRESS_TEMP=""
    AI_SESSION_ID="$session_id"
}

mark_quota_paused() {
    local paused_at

    if "$JQ_BIN" -e '.status == "quota_paused"' "$IN_PROGRESS_FILE" >/dev/null; then
        return 0
    fi
    paused_at="$(date '+%Y-%m-%dT%H:%M:%S%z')"
    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" --arg paused_at "$paused_at" '
        .status = "quota_paused"
        | .quota_paused_at = $paused_at
        | .quota_pause_count = ((.quota_pause_count // 0) + 1)
        | .quota_comment_posted = false
        | .quota_email_sent = false
    ' "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"
    mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
    IN_PROGRESS_TEMP=""
    log "Paused issue #$ISSUE_NUMBER because $SELECTED_AI usage is unavailable; session $AI_SESSION_ID was preserved."
}

post_quota_pause_comment() {
    local issue_number
    local pause_count
    local marker
    local already_posted

    if "$JQ_BIN" -e '.quota_comment_posted // false' "$IN_PROGRESS_FILE" >/dev/null; then
        return 0
    fi
    issue_number="$("$JQ_BIN" -r '.issue_number' "$IN_PROGRESS_FILE")"
    pause_count="$("$JQ_BIN" -r '.quota_pause_count // 1' "$IN_PROGRESS_FILE")"
    marker="<!-- swarm-issue-worker:quota-paused:issue:$issue_number;pause:$pause_count;session:$AI_SESSION_ID -->"
    already_posted="$(
        "$GH_BIN" api --method GET --paginate --slurp \
            "repos/$GITHUB_REPOSITORY/issues/$issue_number/comments" \
            -F per_page=100 \
        | "$JQ_BIN" -r --arg marker "$marker" \
            '[add // [] | .[] | select((.body // "") | contains($marker))] | length'
    )"
    if [ "$already_posted" -eq 0 ]; then
        QUOTA_COMMENT_FILE="$(mktemp "$STATE_DIR/quota-comment.XXXXXX")"
        {
            printf '%s\n' "$marker"
            printf 'Work paused because **%s** no longer has sufficient usage available.\n\n' "$SELECTED_AI"
            printf -- '- Model: `%s`\n' "$SELECTED_MODEL"
            printf -- '- Session: `%s`\n' "$AI_SESSION_ID"
            printf '%s\n' '- The current work and AI session were saved.'
            printf '%s\n' "- The worker will wait for $SELECTED_AI specifically, include any new trusted issue comments, and resume this same session automatically."
        } > "$QUOTA_COMMENT_FILE"
        log "Posting the one-time quota pause notice to GitHub issue #$issue_number."
        "$GH_BIN" issue comment "$issue_number" \
            --repo "$GITHUB_REPOSITORY" \
            --body-file "$QUOTA_COMMENT_FILE"
        rm -f -- "$QUOTA_COMMENT_FILE"
        QUOTA_COMMENT_FILE=""
    fi
    update_in_progress '.quota_comment_posted = true'
}

send_quota_pause_email() {
    local issue_number

    if "$JQ_BIN" -e '.quota_email_sent // false' "$IN_PROGRESS_FILE" >/dev/null; then
        return 0
    fi
    [ -n "$SMTP_CREDENTIALS_FILE" ] || fail "Set SWARM_SMTP_CREDENTIALS_FILE to send the quota pause notification."
    [ -f "$SMTP_CREDENTIALS_FILE" ] || fail "SMTP settings file was not found: $SMTP_CREDENTIALS_FILE"
    [ -n "$SMTP_PASSWORD_INPUT" ] || fail "The SMTP password must be entered through the foreground runner."
    issue_number="$("$JQ_BIN" -r '.issue_number' "$IN_PROGRESS_FILE")"
    log "Sending the one-time quota pause notification for issue #$issue_number."
    printf '%s\n' "$SMTP_PASSWORD_INPUT" | "$PYTHON_BIN" "$SCRIPT_DIR/send_issue_notification.py" \
        --credentials "$SMTP_CREDENTIALS_FILE" \
        --password-stdin \
        --to "$EMAIL_TO" \
        --notification-type quota-paused \
        --issue-number "$issue_number" \
        --issue-title "$("$JQ_BIN" -r '.issue_title' "$IN_PROGRESS_FILE")" \
        --issue-url "$("$JQ_BIN" -r '.issue_url' "$IN_PROGRESS_FILE")" \
        --ai "$SELECTED_AI" \
        --model "$SELECTED_MODEL" \
        --session-id "$AI_SESSION_ID"
    update_in_progress '.quota_email_sent = true'
}

deliver_quota_pause_notifications() {
    post_quota_pause_comment
    send_quota_pause_email
}

mark_quota_resumed() {
    local resumed_at

    resumed_at="$(date '+%Y-%m-%dT%H:%M:%S%z')"
    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" --arg resumed_at "$resumed_at" '
        .status = "active"
        | .quota_resumed_at = $resumed_at
    ' "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"
    mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
    IN_PROGRESS_TEMP=""
}

validate_quota_paused_state() {
    local state_file="$1"

    "$JQ_BIN" -e '
        (.issue_number | type) == "number"
        and (.issue_title | type) == "string"
        and (.issue_url | type) == "string"
        and (.base_sha | type) == "string"
        and (.base_sha | test("^[0-9a-f]{40}$"))
        and ((.candidate_sha? // "") as $sha
            | ($sha | type) == "string"
            and ($sha == "" or ($sha | test("^[0-9a-f]{40}$"))))
        and ((.attempt_start_sha? // "") as $sha
            | ($sha | type) == "string"
            and ($sha == "" or ($sha | test("^[0-9a-f]{40}$"))))
        and (.ai_tool | IN("Claude", "Codex"))
        and (.model | type) == "string" and (.model | length) > 0
        and (.effort | type) == "string" and (.effort | length) > 0
        and (.session_id | type) == "string" and (.session_id | length) > 0
        and .status == "quota_paused"
    ' "$state_file" >/dev/null 2>&1
}

# Resolve recovery commit IDs before they are persisted or trusted. If a
# damaged 40-character ID still has an accurate eight-character prefix, only
# repair it when that prefix identifies exactly one commit between the saved
# base and current main. This recovers a transcription error without guessing
# across unrelated history.
resolve_recovery_candidate_sha() {
    local base_sha="$1"
    local saved_sha="$2"
    local tip_sha="$3"
    local canonical_sha
    local prefix
    local matches

    canonical_sha="$(git -C "$REPO_DIR" rev-parse --verify "$saved_sha^{commit}" 2>/dev/null || true)"
    if [ -n "$canonical_sha" ] \
        && git -C "$REPO_DIR" merge-base --is-ancestor "$base_sha" "$canonical_sha" \
        && git -C "$REPO_DIR" merge-base --is-ancestor "$canonical_sha" "$tip_sha"; then
        printf '%s\n' "$canonical_sha"
        return 0
    fi

    [[ "$saved_sha" =~ ^[0-9a-f]{40}$ ]] || return 1
    prefix="${saved_sha:0:8}"
    matches="$(git -C "$REPO_DIR" rev-list "$tip_sha" "^$base_sha" \
        | awk -v prefix="$prefix" 'index($0, prefix) == 1 { print }')"
    [ "$(printf '%s\n' "$matches" | awk 'NF { count++ } END { print count + 0 }')" -eq 1 ] \
        || return 1
    printf '%s\n' "$matches"
}

normalize_saved_recovery_commits() {
    local state_file="$1"
    local tip_sha="$2"
    local base_sha
    local canonical_base_sha
    local candidate_sha
    local canonical_candidate_sha=""
    local attempt_start_sha
    local canonical_attempt_start_sha=""
    local state_temp

    base_sha="$("$JQ_BIN" -r '.base_sha // empty' "$state_file")"
    canonical_base_sha="$(git -C "$REPO_DIR" rev-parse --verify "$base_sha^{commit}" 2>/dev/null)" \
        || return 1
    git -C "$REPO_DIR" merge-base --is-ancestor "$canonical_base_sha" "$tip_sha" \
        || return 1

    candidate_sha="$("$JQ_BIN" -r '.candidate_sha // empty' "$state_file")"
    if [ -n "$candidate_sha" ]; then
        canonical_candidate_sha="$(resolve_recovery_candidate_sha \
            "$canonical_base_sha" "$candidate_sha" "$tip_sha")" || return 1
    fi

    attempt_start_sha="$("$JQ_BIN" -r '.attempt_start_sha // empty' "$state_file")"
    if [ -n "$attempt_start_sha" ]; then
        canonical_attempt_start_sha="$(git -C "$REPO_DIR" rev-parse --verify \
            "$attempt_start_sha^{commit}" 2>/dev/null)" || return 1
        git -C "$REPO_DIR" merge-base --is-ancestor \
            "$canonical_base_sha" "$canonical_attempt_start_sha" || return 1
        git -C "$REPO_DIR" merge-base --is-ancestor \
            "$canonical_attempt_start_sha" "$tip_sha" || return 1
    fi

    if [ "$base_sha" = "$canonical_base_sha" ] \
        && [ "$candidate_sha" = "$canonical_candidate_sha" ] \
        && [ "$attempt_start_sha" = "$canonical_attempt_start_sha" ]; then
        return 0
    fi

    state_temp="$(mktemp "$STATE_DIR/recovery-state.XXXXXX")"
    if ! "$JQ_BIN" \
        --arg base_sha "$canonical_base_sha" \
        --arg candidate_sha "$canonical_candidate_sha" \
        --arg attempt_start_sha "$canonical_attempt_start_sha" '
            .base_sha = $base_sha
            | (if $candidate_sha != "" then .candidate_sha = $candidate_sha else del(.candidate_sha) end)
            | (if $attempt_start_sha != "" then .attempt_start_sha = $attempt_start_sha else del(.attempt_start_sha) end)
        ' "$state_file" > "$state_temp"; then
        rm -f -- "$state_temp"
        return 1
    fi
    mv -- "$state_temp" "$state_file"
    if [ -n "$candidate_sha" ] && [ "$candidate_sha" != "$canonical_candidate_sha" ]; then
        log "Repaired saved recovery commit $candidate_sha as $canonical_candidate_sha."
    fi
}

drop_worker_stash() {
    local stash_oid="$1"
    local stash_ref

    stash_ref="$(git -C "$REPO_DIR" stash list --format='%H %gd' \
        | awk -v oid="$stash_oid" '$1 == oid { print $2; exit }')"
    if [ -n "$stash_ref" ]; then
        git -C "$REPO_DIR" stash drop "$stash_ref" >/dev/null \
            || log "Warning: restored worker stash $stash_oid could not be dropped."
    fi
}

# Move a quota-paused attempt out of the exclusive active slot. Any dirty
# tracked, staged, or untracked work is shelved first so the next issue starts
# from a clean tree. A commit made before quota ran out remains on main and is
# pinned as candidate_sha, allowing the resumed session to verify or extend it
# even if other issue commits have landed since.
suspend_quota_paused_issue() {
    local issue_number
    local base_sha
    local current_sha
    local attempt_start_sha
    local candidate_sha
    local paused_at
    local stash_oid=""
    local paused_file

    validate_quota_paused_state "$IN_PROGRESS_FILE" \
        || fail "The saved quota-paused session is invalid: $IN_PROGRESS_FILE"
    if ! git -C "$REPO_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        fail "SWARM_REPO_DIR is not a Git repository: $REPO_DIR"
    fi

    current_sha="$(git -C "$REPO_DIR" rev-parse HEAD)"
    normalize_saved_recovery_commits "$IN_PROGRESS_FILE" "$current_sha" \
        || fail "The saved commits for the quota-paused session are invalid or not on main: $IN_PROGRESS_FILE"
    issue_number="$("$JQ_BIN" -r '.issue_number' "$IN_PROGRESS_FILE")"
    base_sha="$("$JQ_BIN" -r '.base_sha' "$IN_PROGRESS_FILE")"
    attempt_start_sha="$("$JQ_BIN" -r '.attempt_start_sha // empty' "$IN_PROGRESS_FILE")"
    candidate_sha="$("$JQ_BIN" -r '.candidate_sha // empty' "$IN_PROGRESS_FILE")"
    paused_at="$("$JQ_BIN" -r '.quota_paused_at // empty' "$IN_PROGRESS_FILE")"
    if ! git -C "$REPO_DIR" merge-base --is-ancestor "$base_sha" "$current_sha"; then
        fail "Main no longer descends from paused issue #$issue_number's saved base commit."
    fi
    mkdir -p -- "$PAUSED_ISSUES_DIR"
    paused_file="$PAUSED_ISSUES_DIR/$issue_number.json"
    if [ -e "$paused_file" ]; then
        fail "A quota-paused state already exists for issue #$issue_number: $paused_file"
    fi

    if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
        git -C "$REPO_DIR" stash push --include-untracked \
            --message "swarm issue worker paused #$issue_number" >/dev/null
        stash_oid="$(git -C "$REPO_DIR" rev-parse refs/stash)"
        if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
            fail "Could not shelve all work for quota-paused issue #$issue_number."
        fi
    fi

    if [ -n "$attempt_start_sha" ]; then
        if [ "$current_sha" != "$attempt_start_sha" ]; then
            candidate_sha="$current_sha"
        fi
    elif [ -z "$candidate_sha" ] && [ -n "$paused_at" ]; then
        # Legacy paused states predate attempt_start_sha. Main may have moved
        # for unrelated work since the pause, so recover the newest commit
        # that existed at the recorded pause time instead of claiming the
        # current HEAD as this issue's candidate.
        candidate_sha="$(git -C "$REPO_DIR" rev-list -1 \
            --before="$paused_at" "$current_sha" 2>/dev/null || true)"
        if [ "$candidate_sha" = "$base_sha" ]; then
            candidate_sha=""
        fi
    fi
    if [ -n "$candidate_sha" ] \
        && ! git -C "$REPO_DIR" merge-base --is-ancestor "$base_sha" "$candidate_sha"; then
        fail "The candidate commit for quota-paused issue #$issue_number is not after its saved base."
    fi

    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" \
        --arg candidate_sha "$candidate_sha" \
        --arg stash_oid "$stash_oid" '
        (if $candidate_sha != "" then .candidate_sha = $candidate_sha else . end)
        | (if $stash_oid != "" then .worktree_stash_oid = $stash_oid else . end)
    ' "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"

    mv -- "$IN_PROGRESS_TEMP" "$paused_file"
    IN_PROGRESS_TEMP=""
    rm -f -- "$IN_PROGRESS_FILE"
    log "Shelved quota-paused issue #$issue_number; other ready issues may now run."
}

restore_quota_paused_issue() {
    local paused_file="$1"
    local issue_number
    local stash_oid
    local resumed_at
    local current_sha

    [ ! -e "$IN_PROGRESS_FILE" ] \
        || fail "Cannot restore a quota-paused issue while another issue is active."
    validate_quota_paused_state "$paused_file" \
        || fail "The saved quota-paused session is invalid: $paused_file"
    if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
        fail "The repository must be clean before restoring a quota-paused issue."
    fi

    current_sha="$(git -C "$REPO_DIR" rev-parse HEAD)"
    normalize_saved_recovery_commits "$paused_file" "$current_sha" \
        || fail "The saved commits for the quota-paused session are invalid or not on main: $paused_file"

    issue_number="$("$JQ_BIN" -r '.issue_number' "$paused_file")"
    stash_oid="$("$JQ_BIN" -r '.worktree_stash_oid // empty' "$paused_file")"
    if [ -n "$stash_oid" ]; then
        if ! git -C "$REPO_DIR" cat-file -e "$stash_oid^{commit}" 2>/dev/null; then
            fail "The shelved work for quota-paused issue #$issue_number is missing: $stash_oid"
        fi
        if ! git -C "$REPO_DIR" stash apply --index "$stash_oid" >/dev/null; then
            fail "Shelved work for issue #$issue_number conflicts with newer issue commits; resolve the worktree manually."
        fi
    fi

    resumed_at="$(date '+%Y-%m-%dT%H:%M:%S%z')"
    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" --arg resumed_at "$resumed_at" '
        .status = "active"
        | .quota_resumed_at = $resumed_at
        | del(.worktree_stash_oid)
    ' "$paused_file" > "$IN_PROGRESS_TEMP"
    mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
    IN_PROGRESS_TEMP=""
    rm -f -- "$paused_file"
    if [ -n "$stash_oid" ]; then
        drop_worker_stash "$stash_oid"
    fi
}

load_paused_issue_numbers() {
    local paused_file

    PAUSED_ISSUES_JSON="[]"
    [ -d "$PAUSED_ISSUES_DIR" ] || return 0
    for paused_file in "$PAUSED_ISSUES_DIR"/*.json; do
        [ -e "$paused_file" ] || continue
        validate_quota_paused_state "$paused_file" \
            || fail "The saved quota-paused session is invalid: $paused_file"
        PAUSED_ISSUES_JSON="$(printf '%s' "$PAUSED_ISSUES_JSON" | "$JQ_BIN" -c \
            --argjson issue_number "$("$JQ_BIN" -r '.issue_number' "$paused_file")" \
            '. + [$issue_number] | unique')"
    done
}

resume_available_paused_issue() {
    local paused_file
    local provider

    [ -d "$PAUSED_ISSUES_DIR" ] || return 1
    for paused_file in "$PAUSED_ISSUES_DIR"/*.json; do
        [ -e "$paused_file" ] || continue
        validate_quota_paused_state "$paused_file" \
            || fail "The saved quota-paused session is invalid: $paused_file"
        provider="$("$JQ_BIN" -r '.ai_tool' "$paused_file")"
        if selected_ai_has_capacity "$provider"; then
            AVAILABLE_PAUSED_FILE="$paused_file"
            if [ "$DRY_RUN" = "1" ]; then
                return 0
            fi
            restore_quota_paused_issue "$paused_file"
            return 0
        fi
    done
    return 1
}

ai_output_indicates_quota() {
    grep -Eqi \
        'usage limit|rate[ _-]?limit|quota|credits? (are )?(exhausted|unavailable)|limit (has been )?reached|hit your .*limit|resets? at|insufficient_quota' \
        "$AI_OUTPUT_FILE" "$AI_DIAGNOSTIC_FILE" 2>/dev/null
}

ai_failure_is_quota() {
    local capacity_status

    if ai_output_indicates_quota; then
        return 0
    fi
    if selected_ai_has_capacity "$SELECTED_AI"; then
        return 1
    else
        capacity_status="$?"
    fi
    # Status 1 is a successful quota query that confirmed insufficient
    # capacity. Status 2 means the query itself failed and must not be turned
    # into a false quota pause/resume cycle.
    [ "$capacity_status" -eq 1 ]
}

extract_followup_metadata() {
    local comments_file="$1"

    "$JQ_BIN" -c \
        --arg trusted_author "$TRUSTED_FOLLOWUP_AUTHOR" '
        def is_worker_generated_comment:
            (.body // "") | contains("<!-- swarm-issue-worker:");
        def is_completion_comment:
            ((.body // "") | contains("<!-- swarm-issue-worker:commit:"))
            and ((.user.login // "") == $trusted_author);
        (add // [] | sort_by(.id)) as $comments
        | ($comments | map(select(is_completion_comment)) | last) as $completion
        | if $completion == null then empty
          else
            (($completion.body
                | capture("swarm-issue-worker:commit:(?<sha>[0-9a-f]{40})")?
                | .sha) // "") as $previous_commit_sha
            | (($completion.body
                | capture("(?:Completed|Reworked) by \\*\\*(?<ai>Claude|Codex)\\*\\*")?
                | .ai) // "") as $previous_ai
            | (($completion.body
                | capture("through-comment:(?<id>[0-9]+)")?
                | .id
                | tonumber) // $completion.id) as $processed_through_id
            | ($comments
                | map(select(
                    (is_worker_generated_comment | not)
                    and .id > $processed_through_id
                    and (.user.login // "") == $trusted_author
                ))) as $followups
            | if ($followups | length) == 0 then empty
              else {
                previous_commit_sha: $previous_commit_sha,
                previous_ai: $previous_ai,
                previous_completion_comment: {
                    id: $completion.id,
                    author: ($completion.user.login // "unknown"),
                    created_at: $completion.created_at,
                    body: ($completion.body // "")
                },
                followup_comments: ($followups | map({
                    id,
                    author: (.user.login // "unknown"),
                    created_at,
                    body: (.body // "")
                })),
                trigger_comment_id: ($followups | last | .id),
                trigger_created_at: ($followups | first | .created_at)
              }
            end
          end
    ' "$comments_file"
}

extract_completion_metadata() {
    local comments_file="$1"

    "$JQ_BIN" -c \
        --arg trusted_author "$TRUSTED_FOLLOWUP_AUTHOR" '
        (add // [] | sort_by(.id))
        | map(select(
            ((.body // "") | contains("<!-- swarm-issue-worker:commit:"))
            and ((.user.login // "") == $trusted_author)
        ))
        | last
        | if . == null then empty
          else {
            commit_sha: (((.body
                | capture("swarm-issue-worker:commit:(?<sha>[0-9a-f]{40})")?
                | .sha) // "")),
            comment_id: .id,
            author: (.user.login // "unknown")
          }
          end
    ' "$comments_file"
}

record_completed_issue_from_comments() {
    local issue_number="$1"
    local comments_file="$2"
    local completion_metadata
    local commit_sha
    local main_sha

    completion_metadata="$(extract_completion_metadata "$comments_file")"
    [ -n "$completion_metadata" ] || return 1
    commit_sha="$(printf '%s' "$completion_metadata" | "$JQ_BIN" -r '.commit_sha')"
    if ! [[ "$commit_sha" =~ ^[0-9a-f]{40}$ ]]; then
        log "Ignoring the trusted completion marker on issue #$issue_number because it has no valid commit SHA."
        return 1
    fi
    main_sha="$(git -C "$REPO_DIR" rev-parse --verify refs/heads/main 2>/dev/null || true)"
    if [ -z "$main_sha" ] \
        || ! git -C "$REPO_DIR" cat-file -e "$commit_sha^{commit}" 2>/dev/null \
        || ! git -C "$REPO_DIR" merge-base --is-ancestor "$commit_sha" "$main_sha"; then
        log "Issue #$issue_number has a trusted completion marker for $commit_sha, but that commit is not on local main; leaving it pending until the repository is synchronized."
        return 1
    fi

    if ! grep -Fxq -- "$issue_number" "$COMPLETED_ISSUES_FILE" 2>/dev/null; then
        printf '%s\n' "$issue_number" >> "$COMPLETED_ISSUES_FILE"
    fi
    clear_in_progress_issue "$issue_number"
    log "Recognized issue #$issue_number as already completed by commit $commit_sha on main; no AI run is needed."
    return 0
}

load_resume_comments() {
    COMMENTS_FILE="$(mktemp "$STATE_DIR/github-comments.XXXXXX")"
    if ! "$GH_BIN" api --method GET --paginate --slurp \
        "repos/$GITHUB_REPOSITORY/issues/$ISSUE_NUMBER/comments" \
        -F per_page=100 > "$COMMENTS_FILE"; then
        fail "GitHub comment query failed while preparing to resume issue #$ISSUE_NUMBER."
    fi
    RESUME_COMMENTS_JSON="$("$JQ_BIN" -c \
        --arg trusted_author "$TRUSTED_FOLLOWUP_AUTHOR" \
        --argjson after_id "$SESSION_COMMENT_ID" '
            def is_worker_comment:
                (.body // "") | contains("<!-- swarm-issue-worker:");
            add // []
            | sort_by(.id)
            | map(select(
                .id > $after_id
                and (is_worker_comment | not)
                and (.user.login // "") == $trusted_author
            ))
            | map({
                id,
                author: (.user.login // "unknown"),
                created_at,
                body: (.body // "")
            })
        ' "$COMMENTS_FILE")"
    rm -f -- "$COMMENTS_FILE"
    COMMENTS_FILE=""
    RESUME_COMMENT_ID="$(printf '%s' "$RESUME_COMMENTS_JSON" | "$JQ_BIN" -r \
        --argjson fallback "$SESSION_COMMENT_ID" \
        'if length > 0 then (last.id) else $fallback end')"
}

save_session_comment_watermark() {
    local comment_id="$1"

    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" --argjson comment_id "$comment_id" \
        '.session_comment_id = $comment_id' "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"
    mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
    IN_PROGRESS_TEMP=""
    SESSION_COMMENT_ID="$comment_id"
}

save_attempt_start_sha() {
    local start_sha="$1"

    IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
    "$JQ_BIN" --arg start_sha "$start_sha" \
        '.attempt_start_sha = $start_sha' "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"
    mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
    IN_PROGRESS_TEMP=""
}

prepare_recovery_repository_state() {
    RECOVERY_HAS_DIRTY_WORKTREE=0

    # A previous attempt can have both produced its issue commit and left (or
    # raced with) unrelated worktree changes. Record the commit independently
    # so dirtiness does not hide a completed implementation during recovery.
    if [ "$RUN_START_SHA" != "$BASE_SHA" ]; then
        if [ -z "$RECOVERY_CANDIDATE_SHA" ]; then
            RECOVERY_CANDIDATE_SHA="$RUN_START_SHA"
            IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
            "$JQ_BIN" --arg candidate_sha "$RECOVERY_CANDIDATE_SHA" \
                '.candidate_sha = $candidate_sha' "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"
            mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
            IN_PROGRESS_TEMP=""
        fi
        log "Verifying commit $RECOVERY_CANDIDATE_SHA as the recovered implementation for issue #$ISSUE_NUMBER."
    fi
    if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
        RECOVERY_HAS_DIRTY_WORKTREE=1
        log "Preserving uncommitted work while recovering issue #$ISSUE_NUMBER."
    elif [ -z "$RECOVERY_CANDIDATE_SHA" ]; then
        log "Retrying issue #$ISSUE_NUMBER from its original clean base commit."
    fi
}

preserve_dirty_worktree_after_completion() {
    if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
        log "Warning: preserving uncommitted work after accepting issue #$ISSUE_NUMBER's commit; new issues will wait for a clean tree."
    fi
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

render_pending_github_comment() {
    local output_file="$1"
    local marker="$2"
    local completion_verb="$3"

    {
        printf '%s\n' "$marker"
        printf '%s by **%s**.\n\n' "$completion_verb" "$("$JQ_BIN" -r '.ai_tool // .ai' "$PENDING_EMAIL_FILE")"
        printf -- '- Model: `%s`\n' "$("$JQ_BIN" -r '.model // "unknown"' "$PENDING_EMAIL_FILE")"
        printf -- '- Effort: `%s`\n' "$("$JQ_BIN" -r '.effort // "unknown"' "$PENDING_EMAIL_FILE")"
        printf -- '- Commit: `%s` — %s\n\n' \
            "$("$JQ_BIN" -r '.commit_sha' "$PENDING_EMAIL_FILE")" \
            "$("$JQ_BIN" -r '.commit_message' "$PENDING_EMAIL_FILE")"
        printf '<details><summary>AI completion summary</summary>\n\n'
        # Keep the AI response as ordinary Markdown. Prefixing it with four
        # spaces made GitHub render every heading and bullet as a code block.
        "$JQ_BIN" -r '.ai_output // "(No captured AI output was available.)"' "$PENDING_EMAIL_FILE"
        printf '\n</details>\n'
    } > "$output_file"
}

post_pending_github_comment() {
    local issue_number
    local commit_sha
    local marker
    local already_posted
    local trigger_comment_id
    local completion_verb

    if "$JQ_BIN" -e '.github_comment_posted // false' "$PENDING_EMAIL_FILE" >/dev/null; then
        return 0
    fi

    issue_number="$("$JQ_BIN" -r '.issue_number' "$PENDING_EMAIL_FILE")"
    commit_sha="$("$JQ_BIN" -r '.commit_sha' "$PENDING_EMAIL_FILE")"
    trigger_comment_id="$("$JQ_BIN" -r '.trigger_comment_id // 0' "$PENDING_EMAIL_FILE")"
    marker="<!-- swarm-issue-worker:commit:$commit_sha"
    if [[ "$trigger_comment_id" =~ ^[1-9][0-9]*$ ]]; then
        marker="$marker;through-comment:$trigger_comment_id"
    fi
    marker="$marker -->"
    already_posted="$(
        "$GH_BIN" api --method GET --paginate --slurp \
            "repos/$GITHUB_REPOSITORY/issues/$issue_number/comments" \
            -F per_page=100 \
        | "$JQ_BIN" -r --arg marker "$marker" \
            '[add // [] | .[] | select((.body // "") | contains($marker))] | length'
    )"

    if [ "$already_posted" -eq 0 ]; then
        GITHUB_COMMENT_FILE="$(mktemp "$STATE_DIR/github-comment.XXXXXX")"
        completion_verb="Completed"
        if [ "$("$JQ_BIN" -r '.work_type // "initial"' "$PENDING_EMAIL_FILE")" = "followup" ]; then
            completion_verb="Reworked"
        fi
        render_pending_github_comment "$GITHUB_COMMENT_FILE" "$marker" "$completion_verb"

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

# Lets the shell regression test load the state-transition helpers without
# acquiring the production lock, querying GitHub, or starting an AI process.
if [ "${SWARM_ISSUE_WORKER_TEST_MODE:-0}" = "1" ]; then
    return 0 2>/dev/null || exit 0
fi

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

# Migrate the single-slot quota-paused state written by the previous worker.
# Once its one-time notifications are complete, either resume it immediately
# or shelve its work and free the active slot so another issue can be selected.
if [ -f "$IN_PROGRESS_FILE" ] \
    && "$JQ_BIN" -e '.status == "quota_paused"' "$IN_PROGRESS_FILE" >/dev/null 2>&1; then
    if ! validate_quota_paused_state "$IN_PROGRESS_FILE"; then
        fail "The saved quota-paused session is invalid: $IN_PROGRESS_FILE"
    fi
    ISSUE_NUMBER="$("$JQ_BIN" -r '.issue_number' "$IN_PROGRESS_FILE")"
    ISSUE_TITLE="$("$JQ_BIN" -r '.issue_title' "$IN_PROGRESS_FILE")"
    ISSUE_URL="$("$JQ_BIN" -r '.issue_url' "$IN_PROGRESS_FILE")"
    SELECTED_AI="$("$JQ_BIN" -r '.ai_tool' "$IN_PROGRESS_FILE")"
    SELECTED_MODEL="$("$JQ_BIN" -r '.model' "$IN_PROGRESS_FILE")"
    SELECTED_EFFORT="$("$JQ_BIN" -r '.effort' "$IN_PROGRESS_FILE")"
    AI_SESSION_ID="$("$JQ_BIN" -r '.session_id' "$IN_PROGRESS_FILE")"
    if [ "$DRY_RUN" != "1" ]; then
        deliver_quota_pause_notifications
    fi
    if ! selected_ai_has_capacity "$SELECTED_AI"; then
        if [ "$DRY_RUN" = "1" ]; then
            log "Dry run: issue #$ISSUE_NUMBER remains quota-paused on $SELECTED_AI."
            exit 0
        fi
        suspend_quota_paused_issue
        exit "$QUOTA_PAUSED_EXIT_CODE"
    fi
    if [ "$DRY_RUN" = "1" ]; then
        log "Dry run: would resume $SELECTED_AI session $AI_SESSION_ID for issue #$ISSUE_NUMBER."
        exit 0
    fi
    mark_quota_resumed
    QUOTA_RESUME_READY=1
    SESSION_IS_RESUME=1
    log "$SELECTED_AI usage is available again; preparing to resume session $AI_SESSION_ID for issue #$ISSUE_NUMBER."
fi

# Paused sessions no longer monopolize in-progress-issue.json. Prefer the
# first one whose pinned provider has recovered; otherwise leave every paused
# state untouched and continue on to fresh/follow-up issue selection.
if [ ! -f "$IN_PROGRESS_FILE" ] && resume_available_paused_issue; then
    if [ "$DRY_RUN" = "1" ]; then
        log "Dry run: would restore quota-paused issue #$("$JQ_BIN" -r '.issue_number' "$AVAILABLE_PAUSED_FILE") with its pinned $("$JQ_BIN" -r '.ai_tool' "$AVAILABLE_PAUSED_FILE") session."
        exit 0
    fi
    ISSUE_NUMBER="$("$JQ_BIN" -r '.issue_number' "$IN_PROGRESS_FILE")"
    ISSUE_TITLE="$("$JQ_BIN" -r '.issue_title' "$IN_PROGRESS_FILE")"
    ISSUE_URL="$("$JQ_BIN" -r '.issue_url' "$IN_PROGRESS_FILE")"
    SELECTED_AI="$("$JQ_BIN" -r '.ai_tool' "$IN_PROGRESS_FILE")"
    SELECTED_MODEL="$("$JQ_BIN" -r '.model' "$IN_PROGRESS_FILE")"
    SELECTED_EFFORT="$("$JQ_BIN" -r '.effort' "$IN_PROGRESS_FILE")"
    AI_SESSION_ID="$("$JQ_BIN" -r '.session_id' "$IN_PROGRESS_FILE")"
    QUOTA_RESUME_READY=1
    SESSION_IS_RESUME=1
    log "$SELECTED_AI usage is available again; restored session $AI_SESSION_ID for issue #$ISSUE_NUMBER."
fi

load_paused_issue_numbers

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
        and ((.work_type // "initial") | IN("initial", "followup"))
        and (if (.work_type // "initial") == "followup" then
            (.previous_commit_sha | type) == "string"
            and (.previous_commit_sha | test("^[0-9a-f]{40}$"))
            and (.previous_completion_comment | type) == "object"
            and (.followup_comments | type) == "array"
            and (.trigger_comment_id | type) == "number"
        else true end)
    ' "$IN_PROGRESS_FILE" >/dev/null 2>&1; then
        fail "The saved in-progress issue state is invalid: $IN_PROGRESS_FILE"
    fi
    IN_PROGRESS_ISSUE_NUMBER_JSON="$("$JQ_BIN" -r '.issue_number' "$IN_PROGRESS_FILE")"
    SAVED_WORK_TYPE="$("$JQ_BIN" -r '.work_type // "initial"' "$IN_PROGRESS_FILE")"
    if [ "$SAVED_WORK_TYPE" = "initial" ] \
        && printf '%s' "$COMPLETED_JSON" | "$JQ_BIN" -e \
        --argjson issue_number "$IN_PROGRESS_ISSUE_NUMBER_JSON" \
        'index($issue_number) != null' >/dev/null; then
        clear_in_progress_issue "$IN_PROGRESS_ISSUE_NUMBER_JSON"
        IN_PROGRESS_ISSUE_NUMBER_JSON="null"
    fi
fi

ASSIGNED_ISSUES_JSON="$("$JQ_BIN" -c \
    --arg assignee "$GITHUB_ASSIGNEE" \
    '
        (add // [])
        | map(select(.pull_request == null))
        | map(select(any(.assignees[]?; .login == $assignee)))
        | sort_by(.created_at)
    ' "$ISSUES_FILE")"

# A trusted completion can be posted outside this worker (for example, an
# interactive Codex session). Reconcile that GitHub marker before selecting a
# fresh issue so a valid commit already on main is never sent through an empty
# recovery loop merely because the local completed-issues file missed it.
while IFS= read -r completion_candidate; do
    completed_issue_number="$(printf '%s' "$completion_candidate" | "$JQ_BIN" -r '.number')"
    COMMENTS_FILE="$(mktemp "$STATE_DIR/github-comments.XXXXXX")"
    if ! "$GH_BIN" api --method GET --paginate --slurp \
        "repos/$GITHUB_REPOSITORY/issues/$completed_issue_number/comments" \
        -F per_page=100 > "$COMMENTS_FILE"; then
        fail "GitHub comment query failed while reconciling issue #$completed_issue_number."
    fi
    if record_completed_issue_from_comments "$completed_issue_number" "$COMMENTS_FILE"; then
        COMPLETED_JSON="$(printf '%s' "$COMPLETED_JSON" | "$JQ_BIN" -c \
            --argjson issue_number "$completed_issue_number" \
            '. + [$issue_number] | unique')"
        if [ "$IN_PROGRESS_ISSUE_NUMBER_JSON" = "$completed_issue_number" ]; then
            IN_PROGRESS_ISSUE_NUMBER_JSON="null"
        fi
    fi
    rm -f -- "$COMMENTS_FILE"
    COMMENTS_FILE=""
done < <(printf '%s' "$ASSIGNED_ISSUES_JSON" | "$JQ_BIN" -c \
    --argjson completed "$COMPLETED_JSON" \
    --argjson paused "$PAUSED_ISSUES_JSON" '
    .[]
    | select(.number as $number | ($completed | index($number)) == null)
    | select(.number as $number | ($paused | index($number)) == null)
')

ISSUE_JSON=""
if [ "$IN_PROGRESS_ISSUE_NUMBER_JSON" != "null" ]; then
    ISSUE_JSON="$(printf '%s' "$ASSIGNED_ISSUES_JSON" | "$JQ_BIN" -c \
        --argjson in_progress "$IN_PROGRESS_ISSUE_NUMBER_JSON" \
        'map(select(.number == $in_progress)) | .[0] // empty')"
else
    FRESH_ISSUE_JSON="$(printf '%s' "$ASSIGNED_ISSUES_JSON" | "$JQ_BIN" -c \
        --argjson completed "$COMPLETED_JSON" \
        --argjson paused "$PAUSED_ISSUES_JSON" '
            map(select(.number as $number | ($completed | index($number)) == null))
            | map(select(.number as $number | ($paused | index($number)) == null))
            | .[0] // empty
        ')"
    FOLLOWUP_ISSUE_JSON=""
    FOLLOWUP_TRIGGER_CREATED_AT=""

    while IFS= read -r completed_issue_json; do
        completed_issue_number="$(printf '%s' "$completed_issue_json" | "$JQ_BIN" -r '.number')"
        COMMENTS_FILE="$(mktemp "$STATE_DIR/github-comments.XXXXXX")"
        if ! "$GH_BIN" api --method GET --paginate --slurp \
            "repos/$GITHUB_REPOSITORY/issues/$completed_issue_number/comments" \
            -F per_page=100 > "$COMMENTS_FILE"; then
            fail "GitHub comment query failed for completed issue #$completed_issue_number."
        fi

        FOLLOWUP_METADATA="$(extract_followup_metadata "$COMMENTS_FILE")"
        rm -f -- "$COMMENTS_FILE"
        COMMENTS_FILE=""

        if [ -n "$FOLLOWUP_METADATA" ]; then
            if ! printf '%s' "$FOLLOWUP_METADATA" | "$JQ_BIN" -e \
                '.previous_commit_sha | test("^[0-9a-f]{40}$")' >/dev/null; then
                fail "The latest worker comment on issue #$completed_issue_number does not contain a valid completion commit."
            fi
            candidate_trigger_created_at="$(printf '%s' "$FOLLOWUP_METADATA" | "$JQ_BIN" -r '.trigger_created_at')"
            if [ -z "$FOLLOWUP_ISSUE_JSON" ] \
                || [[ "$candidate_trigger_created_at" < "$FOLLOWUP_TRIGGER_CREATED_AT" ]]; then
                FOLLOWUP_TRIGGER_CREATED_AT="$candidate_trigger_created_at"
                FOLLOWUP_ISSUE_JSON="$(printf '%s' "$completed_issue_json" | "$JQ_BIN" -c \
                    --argjson followup "$FOLLOWUP_METADATA" '. + {_swarm_followup: $followup}')"
            fi
        fi
    done < <(printf '%s' "$ASSIGNED_ISSUES_JSON" | "$JQ_BIN" -c \
        --argjson completed "$COMPLETED_JSON" \
        --argjson paused "$PAUSED_ISSUES_JSON" '
        .[]
        | select(.number as $number | ($completed | index($number)) != null)
        | select(.number as $number | ($paused | index($number)) == null)
    ')

    if [ -n "$FOLLOWUP_ISSUE_JSON" ]; then
        ISSUE_JSON="$FOLLOWUP_ISSUE_JSON"
    else
        ISSUE_JSON="$FRESH_ISSUE_JSON"
    fi
fi

if [ -z "$ISSUE_JSON" ]; then
    if [ "$IN_PROGRESS_ISSUE_NUMBER_JSON" != "null" ]; then
        fail "Saved in-progress issue #$IN_PROGRESS_ISSUE_NUMBER_JSON is no longer open and assigned to $GITHUB_ASSIGNEE; review $IN_PROGRESS_FILE."
    fi
    if [ "$(printf '%s' "$PAUSED_ISSUES_JSON" | "$JQ_BIN" -r 'length')" -gt 0 ]; then
        log "No other issue can be worked now; quota-paused issues remain safely shelved."
    else
        log "No new issue or follow-up comment assigned to $GITHUB_ASSIGNEE was found."
    fi
    exit 0
fi

ISSUE_NUMBER="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.number')"
ISSUE_TITLE="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.title')"
ISSUE_DESCRIPTION="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.body // ""')"
ISSUE_TAGS="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '[.labels[].name] | join(", ")')"
ISSUE_URL="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '.html_url')"
WORK_TYPE="initial"
PREVIOUS_COMMIT_SHA=""
PREVIOUS_AI=""
PREVIOUS_COMPLETION_COMMENT_JSON="null"
FOLLOWUP_COMMENTS_JSON="[]"
TRIGGER_COMMENT_ID_JSON="null"

if [ -f "$IN_PROGRESS_FILE" ]; then
    WORK_TYPE="$("$JQ_BIN" -r '.work_type // "initial"' "$IN_PROGRESS_FILE")"
    PREVIOUS_COMMIT_SHA="$("$JQ_BIN" -r '.previous_commit_sha // empty' "$IN_PROGRESS_FILE")"
    PREVIOUS_AI="$("$JQ_BIN" -r '.previous_ai // empty' "$IN_PROGRESS_FILE")"
    PREVIOUS_COMPLETION_COMMENT_JSON="$("$JQ_BIN" -c '.previous_completion_comment // null' "$IN_PROGRESS_FILE")"
    FOLLOWUP_COMMENTS_JSON="$("$JQ_BIN" -c '.followup_comments // []' "$IN_PROGRESS_FILE")"
    TRIGGER_COMMENT_ID_JSON="$("$JQ_BIN" -c '.trigger_comment_id // null' "$IN_PROGRESS_FILE")"
elif printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -e '._swarm_followup != null' >/dev/null; then
    WORK_TYPE="followup"
    PREVIOUS_COMMIT_SHA="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '._swarm_followup.previous_commit_sha')"
    PREVIOUS_AI="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -r '._swarm_followup.previous_ai // empty')"
    PREVIOUS_COMPLETION_COMMENT_JSON="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -c '._swarm_followup.previous_completion_comment')"
    FOLLOWUP_COMMENTS_JSON="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -c '._swarm_followup.followup_comments')"
    TRIGGER_COMMENT_ID_JSON="$(printf '%s' "$ISSUE_JSON" | "$JQ_BIN" -c '._swarm_followup.trigger_comment_id')"
fi

if [ "$WORK_TYPE" = "followup" ]; then
    log "Selected issue #$ISSUE_NUMBER for rework after GitHub follow-up comment $TRIGGER_COMMENT_ID_JSON: $ISSUE_TITLE"
else
    log "Selected oldest unprocessed assigned issue: #$ISSUE_NUMBER $ISSUE_TITLE"
fi

CLAUDE_AVAILABLE=0
CODEX_AVAILABLE=0
PINNED_AI=""
if [ -f "$IN_PROGRESS_FILE" ]; then
    PINNED_AI="$("$JQ_BIN" -r '.ai_tool // empty' "$IN_PROGRESS_FILE")"
fi

if [ -n "$PINNED_AI" ]; then
    SELECTED_AI="$PINNED_AI"
    SELECTED_MODEL="$("$JQ_BIN" -r '.model' "$IN_PROGRESS_FILE")"
    SELECTED_EFFORT="$("$JQ_BIN" -r '.effort' "$IN_PROGRESS_FILE")"
    AI_SESSION_ID="$("$JQ_BIN" -r '.session_id // empty' "$IN_PROGRESS_FILE")"
    SESSION_COMMENT_ID="$("$JQ_BIN" -r '.session_comment_id // 0' "$IN_PROGRESS_FILE")"
    if [ -n "$AI_SESSION_ID" ]; then
        SESSION_IS_RESUME=1
    fi
    if [ "$QUOTA_RESUME_READY" -ne 1 ]; then
        PINNED_CAPACITY_STATUS=0
        if selected_ai_has_capacity "$SELECTED_AI"; then
            PINNED_CAPACITY_STATUS=0
        else
            PINNED_CAPACITY_STATUS="$?"
        fi
        if [ "$PINNED_CAPACITY_STATUS" -eq 2 ]; then
            log "Could not verify $SELECTED_AI usage for pinned issue #$ISSUE_NUMBER; leaving its state active and retrying later."
            exit 0
        elif [ "$PINNED_CAPACITY_STATUS" -eq 1 ]; then
            if [ "$DRY_RUN" = "1" ]; then
                log "Dry run: pinned $SELECTED_AI session $AI_SESSION_ID is waiting for usage."
                exit 0
            fi
            if [ -z "$AI_SESSION_ID" ]; then
                fail "The pinned $SELECTED_AI attempt has no resumable session ID."
            fi
            mark_quota_paused
            deliver_quota_pause_notifications
            suspend_quota_paused_issue
            exit "$QUOTA_PAUSED_EXIT_CODE"
        fi
    fi
else
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

    if ! select_available_ai_for_issue; then
        log "Neither Claude nor Codex has at least $MIN_REMAINING_PERCENT% remaining in every active quota window; stopping."
        exit 0
    fi
fi
if [ "$SESSION_IS_RESUME" -eq 1 ]; then
    log "Pinned $SELECTED_AI model $SELECTED_MODEL session $AI_SESSION_ID with effort $SELECTED_EFFORT for this continuation."
else
    log "Selected $SELECTED_AI model $SELECTED_MODEL with effort $SELECTED_EFFORT for this run."
fi

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

if [ "$WORK_TYPE" = "followup" ]; then
    if ! git -C "$REPO_DIR" cat-file -e "$PREVIOUS_COMMIT_SHA^{commit}" 2>/dev/null; then
        fail "The previous completion commit for issue #$ISSUE_NUMBER is not available locally: $PREVIOUS_COMMIT_SHA"
    fi
    if ! git -C "$REPO_DIR" merge-base --is-ancestor "$PREVIOUS_COMMIT_SHA" "$RUN_START_SHA"; then
        fail "Main does not contain issue #$ISSUE_NUMBER's previous completion commit $PREVIOUS_COMMIT_SHA."
    fi
fi

if [ -f "$IN_PROGRESS_FILE" ]; then
    SAVED_ISSUE_NUMBER="$("$JQ_BIN" -r '.issue_number' "$IN_PROGRESS_FILE")"
    if [ "$SAVED_ISSUE_NUMBER" != "$ISSUE_NUMBER" ]; then
        fail "Issue #$SAVED_ISSUE_NUMBER is already in progress; refusing to start issue #$ISSUE_NUMBER."
    fi
    normalize_saved_recovery_commits "$IN_PROGRESS_FILE" "$RUN_START_SHA" \
        || fail "The saved commits for issue #$ISSUE_NUMBER are invalid or not on main."
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

    if [ -z "$PINNED_AI" ]; then
        SESSION_COMMENT_ID=0
        if [[ "$TRIGGER_COMMENT_ID_JSON" =~ ^[0-9]+$ ]]; then
            SESSION_COMMENT_ID="$TRIGGER_COMMENT_ID_JSON"
        fi
        IN_PROGRESS_TEMP="$(mktemp "$STATE_DIR/in-progress.XXXXXX")"
        "$JQ_BIN" \
            --arg ai_tool "$SELECTED_AI" \
            --arg model "$SELECTED_MODEL" \
            --arg effort "$SELECTED_EFFORT" \
            --arg session_id "$AI_SESSION_ID" \
            --argjson session_comment_id "$SESSION_COMMENT_ID" '
                .ai_tool = $ai_tool
                | .model = $model
                | .effort = $effort
                | .session_id = $session_id
                | .session_comment_id = $session_comment_id
                | .status = "active"
                | .quota_pause_count = (.quota_pause_count // 0)
            ' "$IN_PROGRESS_FILE" > "$IN_PROGRESS_TEMP"
        mv -- "$IN_PROGRESS_TEMP" "$IN_PROGRESS_FILE"
        IN_PROGRESS_TEMP=""
        log "Attached the legacy recovery attempt to a persistent $SELECTED_AI session."
    fi

    prepare_recovery_repository_state
else
    if [ -n "$(git -C "$REPO_DIR" status --porcelain)" ]; then
        log "Repository has uncommitted changes unrelated to a saved worker attempt; deferring new issue work until the tree is clean."
        exit 0
    fi
    SESSION_COMMENT_ID=0
    if [[ "$TRIGGER_COMMENT_ID_JSON" =~ ^[0-9]+$ ]]; then
        SESSION_COMMENT_ID="$TRIGGER_COMMENT_ID_JSON"
    fi
    save_in_progress_issue "$ISSUE_NUMBER" "$ISSUE_TITLE" "$ISSUE_URL" "$BASE_SHA"
fi

save_attempt_start_sha "$RUN_START_SHA"

PROMPT_FILE="$(mktemp "$STATE_DIR/issue-prompt.XXXXXX")"
if [ "$SESSION_IS_RESUME" -eq 1 ]; then
    load_resume_comments
    printf '%s\n' \
        "Continue the existing unattended session for GitHub issue #$ISSUE_NUMBER ($ISSUE_TITLE) from exactly where the previous turn stopped when usage became unavailable." \
        "Inspect and preserve the work already present in the repository, finish the implementation and foreground verification, and commit the completed work to main as one commit referencing #$ISSUE_NUMBER. Do not push or ask for interactive input." \
        > "$PROMPT_FILE"
    if [ "$(printf '%s' "$RESUME_COMMENTS_JSON" | "$JQ_BIN" -r 'length')" -gt 0 ]; then
        {
            printf '\nTrusted GitHub comments added since this session last received issue context:\n'
            printf '%s' "$RESUME_COMMENTS_JSON" | "$JQ_BIN" -r '
                .[] | "Comment #\(.id) by @\(.author) at \(.created_at):\n\(.body)\n"
            '
            printf '%s\n' "Treat these comments as additional requirements or corrections for the work you are continuing."
        } >> "$PROMPT_FILE"
        log "Adding trusted issue comments through comment $RESUME_COMMENT_ID to the resumed session."
    fi
    save_session_comment_watermark "$RESUME_COMMENT_ID"
else
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
        > "$PROMPT_FILE"

    if [ "$WORK_TYPE" = "followup" ]; then
        {
            printf '\n%s\n' "Follow-up rework context:"
            printf '%s\n' "This issue was previously worked, but one or more new GitHub comments indicate that it needs another pass. Treat the new comments as refinement or defect feedback. Reinspect the existing implementation, make the additional fix, verify it, and create a new commit on main that references #$ISSUE_NUMBER."
            if [ -n "$PREVIOUS_AI" ] && [ "$SELECTED_AI" != "$PREVIOUS_AI" ]; then
                printf '%s\n' "The previous pass was completed by $PREVIOUS_AI. You are intentionally providing an independent second-provider review; challenge prior assumptions and use the new comments and repository evidence as the source of truth."
            fi
            printf '\nPrevious completion commit and change summary:\n'
            git -C "$REPO_DIR" show --no-ext-diff --format=fuller --stat --summary "$PREVIOUS_COMMIT_SHA"
            printf '\nInspect the complete previous patch with: git show --no-ext-diff %s\n' "$PREVIOUS_COMMIT_SHA"
            printf '\nPrevious worker completion comment:\n'
            printf '%s' "$PREVIOUS_COMPLETION_COMMENT_JSON" | "$JQ_BIN" -r '
                "Comment #\(.id) by @\(.author) at \(.created_at):\n\(.body)"
            '
            printf '\nNew GitHub follow-up comments to address, in order:\n'
            printf '%s' "$FOLLOWUP_COMMENTS_JSON" | "$JQ_BIN" -r '
                .[] | "Comment #\(.id) by @\(.author) at \(.created_at):\n\(.body)\n"
            '
        } >> "$PROMPT_FILE"
    fi
fi

printf '%s\n' \
    "Your final response is shown in the terminal and posted to GitHub as rendered Markdown. Keep it concise and use exactly these headings: '## Summary', '## Changes', '## Verification', and '## Operational notes'. Under Summary, state the outcome and the problem resolved in one short paragraph. Under Changes and Verification, use short bullets. Under Operational notes, state whether the commit was pushed and mention only deployment, restart, migration, or follow-up requirements that actually apply; otherwise write '- None.' Do not include code snippets, diffs, file contents, command transcripts, or step-by-step implementation output." \
    >> "$PROMPT_FILE"

if [ "$RECOVERY_HAS_DIRTY_WORKTREE" -eq 1 ]; then
    printf '%s\n' \
        "The worktree also contains uncommitted changes. Inspect them, but do not assume they belong to this issue: preserve unrelated changes exactly as they are. If any are unfinished work for this issue, finish and commit only that issue work." \
        >> "$PROMPT_FILE"
fi
if [ "$RECOVERY_MODE" -eq 1 ] && [ -n "$RECOVERY_CANDIDATE_SHA" ]; then
    printf '%s\n' \
        "This is a recovery verification run. Commit $RECOVERY_CANDIDATE_SHA was created after the original attempt began and may already implement this issue. Verify the implementation and tests. If it is complete, do not create duplicate code or rewrite history; put SWARM_RECOVERY_COMPLETE on its own final line after your concise summary. If it is incomplete, finish it and create the required commit." \
        >> "$PROMPT_FILE"
fi

: > "$AI_OUTPUT_FILE"
: > "$AI_DIAGNOSTIC_FILE"
AI_STATUS=0

if [ "$SELECTED_AI" = "Claude" ]; then
    CLAUDE_ARGS=(
        --model "$SELECTED_MODEL"
        --effort "$SELECTED_EFFORT"
        --permission-mode bypassPermissions
    )
    if [ "$SESSION_IS_RESUME" -eq 1 ]; then
        CLAUDE_ARGS+=(--resume "$AI_SESSION_ID")
    else
        CLAUDE_ARGS+=(--session-id "$AI_SESSION_ID")
    fi
    set +e
    (
        cd -- "$REPO_DIR"
        "$CLAUDE_BIN" "${CLAUDE_ARGS[@]}" \
            -p - < "$PROMPT_FILE"
    ) 2>&1 | tee "$AI_OUTPUT_FILE"
    AI_STATUS="${PIPESTATUS[0]}"
    set -e
else
    # Current Codex uses these equivalents for the older --yolo and --effort
    # spellings, and '-' reads the prompt from stdin (where -p now means profile).
    log "Codex is working. Detailed implementation output is hidden; its final summary will appear when finished."
    set +e
    if [ "$SESSION_IS_RESUME" -eq 1 ]; then
        (
            cd -- "$REPO_DIR"
            "$CODEX_BIN" exec resume \
                -m "$SELECTED_MODEL" \
                --dangerously-bypass-approvals-and-sandbox \
                --dangerously-bypass-hook-trust \
                -c "model_reasoning_effort=\"$SELECTED_EFFORT\"" \
                --json \
                --output-last-message "$AI_OUTPUT_FILE" \
                "$AI_SESSION_ID" \
                - < "$PROMPT_FILE"
        ) > "$AI_DIAGNOSTIC_FILE" 2>&1
        AI_STATUS="$?"
    else
        "$CODEX_BIN" exec \
            -m "$SELECTED_MODEL" \
            --dangerously-bypass-approvals-and-sandbox \
            --dangerously-bypass-hook-trust \
            -c "model_reasoning_effort=\"$SELECTED_EFFORT\"" \
            -C "$REPO_DIR" \
            --json \
            --output-last-message "$AI_OUTPUT_FILE" \
            - < "$PROMPT_FILE" > "$AI_DIAGNOSTIC_FILE" 2>&1
        AI_STATUS="$?"
    fi
    set -e
    CAPTURED_CODEX_SESSION_ID="$("$JQ_BIN" -Rr '
        fromjson?
        | select(.type == "thread.started")
        | .thread_id // empty
    ' "$AI_DIAGNOSTIC_FILE" | sed -n '1p')"
    if [ -n "$CAPTURED_CODEX_SESSION_ID" ]; then
        save_codex_session_id "$CAPTURED_CODEX_SESSION_ID"
    fi
fi

if [ "$AI_STATUS" -ne 0 ] || [ ! -s "$AI_OUTPUT_FILE" ]; then
    QUOTA_FAILURE=0
    if ai_failure_is_quota; then
        QUOTA_FAILURE=1
    fi
    if [ "$QUOTA_FAILURE" -eq 1 ]; then
        if [ -z "$AI_SESSION_ID" ]; then
            fail "$SELECTED_AI exhausted usage before returning a resumable session ID. Its worktree state was preserved, but the session cannot be resumed automatically."
        fi
        mark_quota_paused
        deliver_quota_pause_notifications
        suspend_quota_paused_issue
        exit "$QUOTA_PAUSED_EXIT_CODE"
    fi
    if [ "$AI_STATUS" -ne 0 ]; then
        fail "$SELECTED_AI exited unsuccessfully. Its session and repository state were preserved; output is in $AI_OUTPUT_FILE and $AI_DIAGNOSTIC_FILE."
    fi
    fail "$SELECTED_AI finished without writing a final summary. Its session and repository state were preserved; diagnostics are in $AI_DIAGNOSTIC_FILE."
fi

if [ "$SELECTED_AI" = "Codex" ]; then
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
preserve_dirty_worktree_after_completion

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
    --arg work_type "$WORK_TYPE" \
    --argjson trigger_comment_id "$TRIGGER_COMMENT_ID_JSON" \
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
            work_type: $work_type,
            trigger_comment_id: $trigger_comment_id,
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
