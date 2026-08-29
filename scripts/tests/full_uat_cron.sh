#!/usr/bin/env bash
# Runs ./scripts/tests/full_uat_suite.sh once a day, at a fixed local time
# (03:00 by default), against whatever code is on `main` at that moment,
# and tracks results in ONE persistent GitHub issue (reused across runs
# while it stays open) instead of filing a fresh issue every time something
# fails — so breaks and their eventual fixes show up as a single timeline
# in one ticket.
#
# Fixed, once-daily scheduling is deliberate, not just a default: if a
# failure ever triggers a bad feedback loop with something else that reacts
# to this tracking issue (e.g. an unrelated automation treating a new
# comment on an assigned issue as "more work to do"), the loop is bounded
# to once per 24 hours instead of firing on every commit. See
# swarm-e2e-suite-lockdown before changing the schedule back to
# commit-triggered checking.
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
# Each day's tick, at the scheduled hour:
#   1. If a previous check/run is still in progress (lock held by a live
#      PID), skip this tick entirely — never run two full_uat_suite.sh
#      invocations at the same time.
#   2. `git fetch origin main` (safe — only updates the remote-tracking ref,
#      never touches the working tree or any local branch) so the
#      not-yet-pushed-commit count below is accurate. This script never
#      pulls/merges/checks out anything on its own; it always tests
#      whatever is currently checked out locally, exactly as-is.
#   3. Compare the current local HEAD commit to the SHA recorded from the
#      last check. If unchanged (nothing landed since yesterday's run),
#      skip — nothing new to test. A changed SHA covers both cases the user
#      cares about: new commits landed on `main` (e.g. a `git pull`/merge
#      brought origin/main forward) and new local commits that haven't been
#      pushed yet.
#   4. Two preconditions, checked only once a real change is pending (so a
#      quiet SHA never triggers a real-server/SMB probe for nothing): the
#      local media server must already be answering its health endpoint
#      (same check tv_e2e_suite.sh/tv_uat_suite.sh already use — this
#      script never starts one either), and the SMB share from
#      batocera.local (real UAT media root storage) must be mounted AND
#      pass a real directory read, not just be listed under `mount` — a
#      dropped SMB connection looks identical to a healthy one until you
#      actually try to read it. Either precondition failing skips this
#      tick WITHOUT recording the pending SHA as tested, so the next
#      passing tick still tests it — nothing silently falls through the
#      cracks while the server/share is down.
#   5. If both preconditions hold: run the full suite (local-only — this
#      script owns GitHub issue creation/reuse itself, not
#      full_uat_suite.sh's own --github-issue path, precisely so it can
#      decide "comment on the existing tracking issue" instead of always
#      filing a new one).
#   6. On failure: find the current open tracking issue (by the issue
#      number persisted in this script's own state file, verified still
#      open via a live `gh issue view` call — never assumed stale) and post
#      a comment with this run's full summary; if none is open (never
#      created, or the previous one was closed), file a brand-new issue and
#      remember its number as the new tracking issue. Then, if a Claude or
#      Codex CLI has spare quota, ask it (read-only — no file edits, no
#      shell access) to post a plain-text triage comment on that same
#      issue: likely root cause and where to look. Alternates which
#      provider gets asked, the same "prefer whichever one didn't run last
#      time" rule scripts/issue_worker/swarm_issue_worker.py uses for
#      follow-up passes, falling back to the other if the preferred one is
#      over quota, and skipping the triage step entirely (silently, no
#      GitHub noise) if neither has capacity right now.
#   7. On a clean pass: if a tracking issue is currently open, post a
#      "back to green" comment on it too, so the fix is visible in the same
#      thread as the break — but never create a new issue for a pass, and
#      never run AI triage for a pass (nothing to diagnose).
#
# State lives under .run/full-uat-cron/ (gitignored): state (last-tested
# SHA, tracking issue number, and last AI triage provider), a lock file, a
# rolling log, and each run's captured output. Change policy: same standing
# rule as the suites it wraps — read swarm-e2e-suite-lockdown before
# changing the detection, locking, precondition, or issue-reuse/triage
# logic here.
#
# Usage:
#   ./scripts/tests/full_uat_cron.sh                 # run forever, once a day at 03:00 local time
#   SWARM_FULL_UAT_CRON_HOUR=5 ./scripts/tests/full_uat_cron.sh   # run at 05:00 local time instead
#   SWARM_FULL_UAT_CRON_INTERVAL=300 ./scripts/tests/full_uat_cron.sh   # TESTING ONLY: replaces the daily schedule with a fixed interval (seconds) between checks, so you don't have to wait for 3am to verify this script itself still works
#   ./scripts/tests/full_uat_cron.sh --once           # run exactly one check-and-maybe-test cycle immediately, then exit (no schedule, no loop)
#
# Env vars:
#   SWARM_FULL_UAT_CRON_HOUR      local hour (0-23) to run at once a day (default 3 = 3am)
#   SWARM_FULL_UAT_CRON_INTERVAL  TESTING ONLY — if set, replaces the daily schedule above with a fixed seconds-between-checks interval instead. Leave unset for real use.
#   SWARM_GITHUB_REPOSITORY       where the tracking issue lives (default DotNetRockStar/swarm)
#   SWARM_E2E_ISSUE_LABEL         label applied to a newly-filed tracking issue (default "Testing")
#   SWARM_RUN_DIR                 shared local run directory (default .run)
#   SWARM_STUN_PORT               local rendezvous HTTP port to health-check (default 8080) — same var tv_e2e_suite.sh/tv_uat_suite.sh already use
#   SWARM_UAT_BATOCERA_HOST       SMB host whose mount is required before testing (default batocera.local)
#   SWARM_UAT_TRIAGE_ENABLED      1 to run AI triage on failure, 0 to only post the raw report (default 1)
#   SWARM_MIN_REMAINING_PERCENT   minimum remaining Claude/Codex quota (session+week / rate-limit windows) required to use a provider for triage (default 10) — same var/default swarm_issue_worker.py uses
#   SWARM_CLAUDE_MODEL / SWARM_CLAUDE_EFFORT   Claude model+effort for triage (default claude-sonnet-5 / high) — same vars swarm_issue_worker.py uses
#   SWARM_CODEX_MODEL / SWARM_CODEX_EFFORT     Codex model+effort for triage (default gpt-5.6-sol / high) — same vars swarm_issue_worker.py uses
#   CLAUDE_BIN / CODEX_BIN         executables to invoke (default: whatever `claude`/`codex` resolve to on PATH)

set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# Empty by default (real daily-at-SCHEDULE_HOUR scheduling); set only to
# switch to the old fixed-interval loop for testing this script itself.
CHECK_INTERVAL_SECONDS="${SWARM_FULL_UAT_CRON_INTERVAL:-}"
SCHEDULE_HOUR="${SWARM_FULL_UAT_CRON_HOUR:-3}"
GITHUB_REPOSITORY="${SWARM_GITHUB_REPOSITORY:-DotNetRockStar/swarm}"
ISSUE_LABEL="${SWARM_E2E_ISSUE_LABEL:-Testing}"
RUN_DIR="${SWARM_RUN_DIR:-.run}"
STUN_PORT="${SWARM_STUN_PORT:-8080}"
BATOCERA_HOST="${SWARM_UAT_BATOCERA_HOST:-batocera.local}"
TRIAGE_ENABLED="${SWARM_UAT_TRIAGE_ENABLED:-1}"
# Exported (not just set) because the quota-check functions below hand it
# to an inline `python3 -c` via os.environ rather than shell-interpolating
# it into the Python source.
export MIN_REMAINING_PERCENT="${SWARM_MIN_REMAINING_PERCENT:-10}"
CLAUDE_BIN="${CLAUDE_BIN:-$(command -v claude 2>/dev/null || true)}"
CODEX_BIN="${CODEX_BIN:-$(command -v codex 2>/dev/null || true)}"
CLAUDE_TRIAGE_MODEL="${SWARM_CLAUDE_MODEL:-claude-sonnet-5}"
CLAUDE_TRIAGE_EFFORT="${SWARM_CLAUDE_EFFORT:-high}"
CODEX_TRIAGE_MODEL="${SWARM_CODEX_MODEL:-gpt-5.6-sol}"
CODEX_TRIAGE_EFFORT="${SWARM_CODEX_EFFORT:-high}"

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

# Prints "<seconds-to-wait>|<target timestamp>" for the next occurrence of
# SCHEDULE_HOUR:00:00 local time — today's if it hasn't passed yet,
# otherwise tomorrow's. python3 (not `date`) so this doesn't have to juggle
# BSD- vs GNU-date flag differences across platforms.
next_scheduled_run() {
    python3 -c "
from datetime import datetime, timedelta
import sys
hour = int(sys.argv[1])
now = datetime.now()
target = now.replace(hour=hour, minute=0, second=0, microsecond=0)
if target <= now:
    target += timedelta(days=1)
print(f'{int((target - now).total_seconds())}|{target:%Y-%m-%d %H:%M:%S}')
" "$SCHEDULE_HOUR"
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
STATE_LAST_TRIAGE_PROVIDER=""

load_state() {
    STATE_LAST_SHA=""
    STATE_ISSUE_NUMBER=""
    STATE_LAST_TRIAGE_PROVIDER=""
    [ -f "$STATE_FILE" ] || return 0
    while IFS='=' read -r key value; do
        case "$key" in
            last_tested_sha) STATE_LAST_SHA="$value" ;;
            tracking_issue_number) STATE_ISSUE_NUMBER="$value" ;;
            last_triage_provider) STATE_LAST_TRIAGE_PROVIDER="$value" ;;
        esac
    done < "$STATE_FILE"
}

save_state() {
    {
        echo "last_tested_sha=$STATE_LAST_SHA"
        echo "tracking_issue_number=$STATE_ISSUE_NUMBER"
        echo "last_triage_provider=$STATE_LAST_TRIAGE_PROVIDER"
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

# Same check tv_e2e_suite.sh/tv_uat_suite.sh already run as their own
# preflight — mirrored here so this script can skip the whole tick (and
# the build/gradle cost of even attempting a run) before invoking them,
# instead of letting each wrapped suite discover the same thing on its own.
media_server_running() {
    curl -s -o /dev/null -w '' --max-time 3 "http://127.0.0.1:$STUN_PORT/health"
}

# Real-read health check, not just presence — mirrors the project's own
# "a network mount can remain listed while every read fails" philosophy
# (see settings::media_root_health / start_media_root_recovery in
# apps/server/src/lib.rs). macOS-specific `mount` output parsing
# (`//user@host/share on /Volumes/X (smbfs, ...)`), consistent with this
# repo's desktop-development-on-macOS assumption elsewhere.
smb_share_connected() {
    local mount_point
    mount_point="$(mount | grep -i "$BATOCERA_HOST" | head -1 | sed -E 's/^.* on (.+) \([^)]*\)$/\1/')"
    if [ -z "$mount_point" ]; then
        log "No SMB mount from $BATOCERA_HOST found (checked \`mount\`)."
        return 1
    fi
    if ! ls "$mount_point" >/dev/null 2>&1; then
        log "SMB mount from $BATOCERA_HOST at $mount_point did not respond to a real directory read."
        return 1
    fi
    return 0
}

# Bash port of swarm_issue_worker.py's claude_capacity() — same command,
# same /usage output parsing, same threshold — so "has capacity" means the
# same thing here as it does for the issue worker.
claude_capacity() {
    [ -n "$CLAUDE_BIN" ] || { log "Claude capacity check: claude not found in PATH."; return 2; }
    local usage_json
    if ! usage_json="$("$CLAUDE_BIN" -p "/usage" --output-format json --tools "" --no-session-persistence 2>>"$LOG_FILE")"; then
        log "Claude capacity check: /usage command failed."
        return 2
    fi
    printf '%s' "$usage_json" | python3 -c "
import json, os, re, sys
try:
    payload = json.load(sys.stdin)
    usage = str(payload.get('result') or '')
except Exception:
    sys.exit(2)
session = re.search(r'^Current session:\s*([0-9.]+)% used', usage, re.MULTILINE)
week = re.search(r'^Current week(?: \([^)]*\))?:\s*([0-9.]+)% used', usage, re.MULTILINE)
if not session or not week:
    sys.exit(2)
session_remaining = 100 - float(session.group(1))
week_remaining = 100 - float(week.group(1))
min_remaining = float(os.environ['MIN_REMAINING_PERCENT'])
print(f'Claude remaining quota — session: {session_remaining:g}%; week: {week_remaining:g}%.', file=sys.stderr)
sys.exit(0 if (session_remaining >= min_remaining and week_remaining >= min_remaining) else 1)
" 2>>"$LOG_FILE"
}

# Bash port of swarm_issue_worker.py's codex_capacity() — same script,
# same availability formula.
codex_capacity() {
    [ -n "$CODEX_BIN" ] || { log "Codex capacity check: codex not found in PATH."; return 2; }
    local limits_json
    if ! limits_json="$(python3 "$REPO_ROOT/scripts/issue_worker/codex_rate_limits.py" --codex-bin "$CODEX_BIN" 2>>"$LOG_FILE")"; then
        log "Codex capacity check: rate-limit request failed."
        return 2
    fi
    printf '%s' "$limits_json" | python3 -c "
import json, os, sys
try:
    limits = json.load(sys.stdin)
    windows = [limits.get(k) for k in ('primary', 'secondary') if limits.get(k) is not None]
    used = [float(w['usedPercent']) for w in windows]
except Exception:
    sys.exit(2)
if not used:
    sys.exit(2)
min_remaining = float(os.environ['MIN_REMAINING_PERCENT'])
available = (
    limits.get('rateLimitReachedType') is None
    and not bool(limits.get('spendControlReached', False))
    and all(100 - amount >= min_remaining for amount in used)
)
summary = '; '.join(
    f'{k}: {100 - float(limits[k][\"usedPercent\"]):g}%'
    for k in ('primary', 'secondary') if limits.get(k) is not None
)
print(f'Codex remaining quota — {summary}.', file=sys.stderr)
sys.exit(0 if available else 1)
" 2>>"$LOG_FILE"
}

# Prints "claude" or "codex" on stdout and returns 0 if a provider with
# capacity was found; returns 1 with nothing printed if neither has spare
# quota right now. Alternates from whichever provider ran the previous
# triage — same "prefer the one that didn't run last time" rule
# swarm_issue_worker.py's choose_provider() applies to follow-up passes —
# falling back to the other provider if the preferred one is over quota,
# and preferring Claude first when there is no previous triage yet.
choose_triage_provider() {
    local preferred="claude"
    case "$STATE_LAST_TRIAGE_PROVIDER" in
        claude) preferred="codex" ;;
        codex) preferred="claude" ;;
    esac
    local fallback="codex"
    [ "$preferred" = "codex" ] && fallback="claude"

    local provider
    for provider in "$preferred" "$fallback"; do
        if [ "$provider" = "claude" ]; then
            claude_capacity && { echo "claude"; return 0; }
        else
            codex_capacity && { echo "codex"; return 0; }
        fi
    done
    return 1
}

build_triage_prompt() {
    local prompt_file="$1" sha="$2" summary_path="$3" run_log="$4"
    {
        echo "You are triaging an automated UAT test-suite failure for the SWARM project (a peer-to-peer media streaming suite: Rust desktop media server + Amazon Fire TV client)."
        echo
        echo "Read-only triage ONLY: do not edit, create, or delete any files, and do not run any state-changing commands. Analyze the failure evidence below (Read/Grep the repository for more context if useful) and write a concise, actionable analysis: likely root cause, which file(s)/component(s) are implicated, and suggested next steps for whoever picks this up. This text is posted directly as a GitHub issue comment — write it in that voice, no \"I will now analyze...\" preamble and no meta-commentary about being an AI."
        echo
        echo "Commit under test: $sha"
        echo
        echo "## Failure evidence"
        echo
        if [ -n "$summary_path" ] && [ -f "$summary_path" ]; then
            cat "$summary_path"
        else
            cat "$run_log"
        fi
    } > "$prompt_file"
}

# Deliberately read-only: --sandbox read-only for Codex, and
# --disallowedTools blocking every file-mutating/shell-executing tool for
# Claude — this step only writes a comment, it must never be able to touch
# the repository or run arbitrary commands on this machine.
run_ai_triage() {
    local provider="$1" prompt_file="$2" output_file="$3"
    if [ "$provider" = "claude" ]; then
        "$CLAUDE_BIN" \
            --model "$CLAUDE_TRIAGE_MODEL" \
            --effort "$CLAUDE_TRIAGE_EFFORT" \
            --permission-mode bypassPermissions \
            --disallowedTools "Edit Write NotebookEdit Bash WebFetch WebSearch" \
            -p - < "$prompt_file" > "$output_file" 2>>"$LOG_FILE"
        return $?
    fi
    "$CODEX_BIN" exec \
        -m "$CODEX_TRIAGE_MODEL" \
        --sandbox read-only \
        -c "model_reasoning_effort=\"$CODEX_TRIAGE_EFFORT\"" \
        -C "$REPO_ROOT" \
        --output-last-message "$output_file" \
        - < "$prompt_file" > /dev/null 2>>"$LOG_FILE"
    return $?
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

        if [ "$TRIAGE_ENABLED" = "1" ] && [ -n "$STATE_ISSUE_NUMBER" ]; then
            local provider
            if provider="$(choose_triage_provider)"; then
                log "Running AI triage via $provider on issue #$STATE_ISSUE_NUMBER ..."
                local prompt_file="$STATE_DIR/triage-prompt.md"
                local triage_output="$STATE_DIR/triage-output.md"
                : > "$triage_output"
                build_triage_prompt "$prompt_file" "$sha" "$summary_path" "$run_log"
                if run_ai_triage "$provider" "$prompt_file" "$triage_output" && [ -s "$triage_output" ]; then
                    local triage_body="$STATE_DIR/triage-comment.md"
                    {
                        echo "## AI triage ($provider)"
                        echo
                        cat "$triage_output"
                    } > "$triage_body"
                    if gh issue comment "$STATE_ISSUE_NUMBER" --repo "$GITHUB_REPOSITORY" --body-file "$triage_body" >/dev/null 2>>"$LOG_FILE"; then
                        log "Posted $provider triage on issue #$STATE_ISSUE_NUMBER."
                        STATE_LAST_TRIAGE_PROVIDER="$provider"
                        save_state
                    else
                        log "Could not post $provider triage comment."
                    fi
                else
                    log "AI triage via $provider produced no output or failed — skipping the triage comment for this run."
                fi
            else
                log "No AI provider (Claude/Codex) has spare quota right now — skipping AI triage for this failure."
            fi
        fi
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
    log "Change detected: ${STATE_LAST_SHA:-<none yet>} -> ${current_sha:0:12}$ahead_note."

    # Checked only now (a real change is pending), and deliberately NOT
    # recording current_sha into state on either failure below — the same
    # pending commit is retried on the next tick once the precondition
    # clears, instead of being silently treated as "tested."
    if ! media_server_running; then
        log "Media server is not running (no response from http://127.0.0.1:$STUN_PORT/health) — skipping this tick. Will retry the pending commit next check."
        rm -f "$LOCK_FILE"
        return 0
    fi
    if ! smb_share_connected; then
        log "SMB share from $BATOCERA_HOST is not connected/reachable — skipping this tick. Will retry the pending commit next check."
        rm -f "$LOCK_FILE"
        return 0
    fi

    log "Preconditions met. Running full_uat_suite.sh ..."
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

if [ "$RUN_ONCE" -eq 1 ]; then
    log "full_uat_cron.sh started (pid $$, --once)."
    run_one_check
    exit 0
fi

if [ -n "$CHECK_INTERVAL_SECONDS" ]; then
    log "full_uat_cron.sh started (pid $$, TESTING MODE: fixed interval, checking every ${CHECK_INTERVAL_SECONDS}s — set SWARM_FULL_UAT_CRON_INTERVAL only for testing this script; leave it unset for the real once-a-day schedule)."
    while true; do
        run_one_check
        sleep "$CHECK_INTERVAL_SECONDS"
    done
fi

log "full_uat_cron.sh started (pid $$, scheduled once a day at ${SCHEDULE_HOUR}:00 local time)."
while true; do
    schedule="$(next_scheduled_run)"
    wait_seconds="${schedule%%|*}"
    next_at="${schedule#*|}"
    log "Next check at $next_at local time (in ${wait_seconds}s)."
    sleep "$wait_seconds"
    run_one_check
done
