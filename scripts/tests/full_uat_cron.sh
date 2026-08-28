#!/usr/bin/env bash
# Runs ./scripts/tests/full_uat_suite.sh automatically whenever the code on
# `main` changes, checking every 60 minutes, and tracks results in ONE
# persistent GitHub issue (reused across runs while it stays open) instead
# of filing a fresh issue every time something fails — so breaks and their
# eventual fixes show up as a single timeline in one ticket.
#
# This is a persistent foreground process, run directly in a terminal —
# deliberately NOT installed as a real system cron/launchd job, so it's
# always visible and a plain Ctrl+C stops it (mid-sleep or mid-run; a
# running full_uat_suite.sh child gets killed along with it and the lock
# below is cleaned up immediately, not just left to self-heal on the next
# start). Leave it running in its own terminal/tmux/screen session:
#
#   ./scripts/tests/full_uat_cron.sh
#
# Only background it (nohup) if you specifically don't want it tied to a
# terminal session:
#
#   nohup ./scripts/tests/full_uat_cron.sh >> .run/full-uat-cron/nohup.log 2>&1 &
#
# Each tick:
#   1. If a previous check/run is still in progress (lock held by a live
#      PID), skip this tick entirely — never run two full_uat_suite.sh
#      invocations at the same time.
#   2. `git fetch origin main` (safe — only updates the remote-tracking ref,
#      never touches the working tree or any local branch) so the
#      not-yet-pushed-commit count below is accurate. This script never
#      pulls/merges/checks out anything on its own; it always tests
#      whatever is currently checked out locally, exactly as-is.
#   3. Compare the current local HEAD commit to the SHA recorded from the
#      last check. If unchanged, skip — nothing new to test. A changed SHA
#      covers both cases the user cares about: new commits landed on `main`
#      (e.g. a `git pull`/merge brought origin/main forward) and new local
#      commits that haven't been pushed yet.
#   4. If changed: run the full suite (local-only — this script owns
#      GitHub issue creation/reuse itself, not full_uat_suite.sh's own
#      --github-issue path, precisely so it can decide "comment on the
#      existing tracking issue" instead of always filing a new one).
#   5. On failure: find the current open tracking issue (by the issue
#      number persisted in this script's own state file, verified still
#      open via a live `gh issue view` call — never assumed stale) and post
#      a comment with this run's full summary; if none is open (never
#      created, or the previous one was closed), file a brand-new issue and
#      remember its number as the new tracking issue.
#   6. On a clean pass: if a tracking issue is currently open, post a
#      "back to green" comment on it too, so the fix is visible in the same
#      thread as the break — but never create a new issue for a pass.
#
# State lives under .run/full-uat-cron/ (gitignored): state (last-tested
# SHA + tracking issue number), a lock file, a rolling log, and each run's
# captured output. Change policy: same standing rule as the suites it
# wraps — read swarm-e2e-suite-lockdown before changing the detection,
# locking, or issue-reuse logic here.
#
# Usage:
#   ./scripts/tests/full_uat_cron.sh                 # run forever, checking every 60 minutes
#   SWARM_FULL_UAT_CRON_INTERVAL=300 ./scripts/tests/full_uat_cron.sh   # override the interval (seconds), e.g. for testing this script itself
#   ./scripts/tests/full_uat_cron.sh --once           # run exactly one check-and-maybe-test cycle, then exit (no loop)
#
# Env vars:
#   SWARM_FULL_UAT_CRON_INTERVAL  seconds between checks (default 3600 = 60 minutes)
#   SWARM_GITHUB_REPOSITORY       where the tracking issue lives (default DotNetRockStar/swarm)
#   SWARM_E2E_ISSUE_LABEL         label applied to a newly-filed tracking issue (default "Testing")
#   SWARM_RUN_DIR                 shared local run directory (default .run)

set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

CHECK_INTERVAL_SECONDS="${SWARM_FULL_UAT_CRON_INTERVAL:-3600}"
GITHUB_REPOSITORY="${SWARM_GITHUB_REPOSITORY:-DotNetRockStar/swarm}"
ISSUE_LABEL="${SWARM_E2E_ISSUE_LABEL:-Testing}"
RUN_DIR="${SWARM_RUN_DIR:-.run}"

STATE_DIR="$RUN_DIR/full-uat-cron"
STATE_FILE="$STATE_DIR/state"
LOCK_FILE="$STATE_DIR/lock"
LOG_FILE="$STATE_DIR/cron.log"
mkdir -p "$STATE_DIR"

RUN_ONCE=0
for arg in "$@"; do
    case "$arg" in
        --once) RUN_ONCE=1 ;;
        -h|--help) sed -n '2,45p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown argument: $arg" >&2; exit 2 ;;
    esac
done

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$LOG_FILE"
}

# Ctrl+C (or a plain `kill`) should stop this cleanly and immediately free
# the lock — otherwise a manually-cancelled run would leave a stale lock
# behind. `kill -0` in lock_is_held() would eventually self-heal that once
# this PID is gone anyway, but there's no reason to wait for that.
trap 'log "Interrupted — releasing lock and exiting."; rm -f "$LOCK_FILE"; exit 130' INT TERM

lock_is_held() {
    [ -f "$LOCK_FILE" ] || return 1
    local pid
    pid="$(cat "$LOCK_FILE" 2>/dev/null || true)"
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
}

STATE_LAST_SHA=""
STATE_ISSUE_NUMBER=""

load_state() {
    STATE_LAST_SHA=""
    STATE_ISSUE_NUMBER=""
    [ -f "$STATE_FILE" ] || return 0
    while IFS='=' read -r key value; do
        case "$key" in
            last_tested_sha) STATE_LAST_SHA="$value" ;;
            tracking_issue_number) STATE_ISSUE_NUMBER="$value" ;;
        esac
    done < "$STATE_FILE"
}

save_state() {
    {
        echo "last_tested_sha=$STATE_LAST_SHA"
        echo "tracking_issue_number=$STATE_ISSUE_NUMBER"
    } > "$STATE_FILE"
}

# Returns the currently open tracking issue's number on stdout, or nothing.
# Trusts the persisted number only after confirming — right now, live —
# that the issue still exists and is still open; a closed/deleted/never-set
# issue always falls through to "none", which is exactly what makes "file a
# new issue after the old one was closed" work correctly.
find_open_tracking_issue() {
    [ -n "$STATE_ISSUE_NUMBER" ] || return 0
    local state
    state="$(gh issue view "$STATE_ISSUE_NUMBER" --repo "$GITHUB_REPOSITORY" --json state -q .state 2>/dev/null || true)"
    [ "$state" = "OPEN" ] && echo "$STATE_ISSUE_NUMBER"
}

handle_result() {
    local sha="$1" ahead_count="$2" total_fail="$3" summary_path="$4" run_log="$5"

    if ! command -v gh >/dev/null 2>&1; then
        log "gh CLI not available — result not posted to GitHub (total_fail=$total_fail; see $run_log)."
        return 0
    fi

    local open_issue
    open_issue="$(find_open_tracking_issue)"
    local body_file="$STATE_DIR/comment-body.md"

    if [ "$total_fail" -gt 0 ]; then
        {
            echo "## Full UAT run — $(date '+%Y-%m-%d %H:%M:%S %Z') — commit \`${sha:0:12}\`"
            echo
            if [ "$ahead_count" -gt 0 ]; then
                echo "_This commit is $ahead_count commit(s) ahead of \`origin/main\` — not yet pushed._"
                echo
            fi
            echo "**Result: FAILED — $total_fail failure(s)**"
            echo
            if [ -n "$summary_path" ] && [ -f "$summary_path" ]; then
                cat "$summary_path"
            else
                echo "(could not locate full_uat_suite.sh's own summary.md — raw captured output follows)"
                echo
                echo '```'
                cat "$run_log"
                echo '```'
            fi
        } > "$body_file"

        if [ -n "$open_issue" ]; then
            log "FAIL ($total_fail). Commenting on existing tracking issue #$open_issue."
            gh issue comment "$open_issue" --repo "$GITHUB_REPOSITORY" --body-file "$body_file" >/dev/null
            STATE_ISSUE_NUMBER="$open_issue"
        else
            log "FAIL ($total_fail). No open tracking issue — filing a new one."
            local title="Full SWARM UAT failing as of $(date '+%Y-%m-%d %H:%M')"
            local new_url
            if new_url="$(gh issue create --repo "$GITHUB_REPOSITORY" --title "$title" --body-file "$body_file" --label "$ISSUE_LABEL" 2>&1)"; then
                log "Filed: $new_url"
                STATE_ISSUE_NUMBER="$(basename "$new_url")"
            else
                log "Could not file a GitHub issue: $new_url"
            fi
        fi
        save_state
    else
        log "PASS (commit ${sha:0:12})."
        if [ -n "$open_issue" ]; then
            {
                echo "## Full UAT run — $(date '+%Y-%m-%d %H:%M:%S %Z') — commit \`${sha:0:12}\`"
                echo
                echo "**Result: PASSED** — every suite clean. Close this issue if you're satisfied this is resolved; the next real failure opens a new tracking issue."
            } > "$body_file"
            log "Posting recovery update on tracking issue #$open_issue."
            gh issue comment "$open_issue" --repo "$GITHUB_REPOSITORY" --body-file "$body_file" >/dev/null
        fi
    fi
}

run_one_check() {
    if lock_is_held; then
        log "Previous check/run still in progress (pid $(cat "$LOCK_FILE" 2>/dev/null)) — skipping this tick."
        return 0
    fi
    echo "$$" > "$LOCK_FILE"

    load_state

    if ! git fetch origin main >/dev/null 2>&1; then
        log "warning: 'git fetch origin main' failed (offline?) — the not-yet-pushed count below may be stale."
    fi

    local current_sha
    current_sha="$(git rev-parse HEAD)"

    if [ "$current_sha" = "$STATE_LAST_SHA" ]; then
        log "No change since last check (still at ${current_sha:0:12}). Skipping."
        rm -f "$LOCK_FILE"
        return 0
    fi

    local ahead_count
    ahead_count="$(git rev-list --count origin/main..HEAD 2>/dev/null || echo 0)"

    local ahead_note=""
    [ "$ahead_count" -gt 0 ] && ahead_note=" ($ahead_count commit(s) not yet pushed)"
    log "Change detected: ${STATE_LAST_SHA:-<none yet>} -> ${current_sha:0:12}$ahead_note. Running full_uat_suite.sh ..."

    local run_log="$STATE_DIR/run-$(date +%Y%m%d-%H%M%S).log"
    ./scripts/tests/full_uat_suite.sh > "$run_log" 2>&1
    local suite_status=$?

    STATE_LAST_SHA="$current_sha"
    save_state

    local summary_path total_fail
    summary_path="$(grep -oE '^==> Summary written to .*$' "$run_log" 2>/dev/null | tail -1 | sed 's/^==> Summary written to //')"
    total_fail="$(grep -oE 'FAIL: [0-9]+ +SKIP:' "$run_log" 2>/dev/null | tail -1 | grep -oE '[0-9]+')"
    if [ -z "$total_fail" ]; then
        # Couldn't parse a totals line at all (e.g. every suite was skipped
        # via env config, or a hard crash before the summary was written) —
        # fall back to the orchestrator's own exit code so a real problem
        # is never silently treated as a pass.
        if [ "$suite_status" -ne 0 ]; then total_fail=1; else total_fail=0; fi
    fi

    handle_result "$current_sha" "$ahead_count" "$total_fail" "$summary_path" "$run_log"

    rm -f "$LOCK_FILE"
}

log "full_uat_cron.sh started (pid $$, checking every ${CHECK_INTERVAL_SECONDS}s)."

if [ "$RUN_ONCE" -eq 1 ]; then
    run_one_check
    exit 0
fi

while true; do
    run_one_check
    sleep "$CHECK_INTERVAL_SECONDS"
done
