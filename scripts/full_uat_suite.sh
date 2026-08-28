#!/usr/bin/env bash
# Master orchestrator for scripts/TV_TESTING.md's "run everything" flow:
# runs the media server backend UAT suite, the TV E2E smoke test, and the
# full TV UAT suite in sequence, captures each one's own evidence, and
# — only when at least one of them reported a real failure — offers to file
# ONE consolidated GitHub issue covering every suite's result (passes and
# failures both), instead of the up-to-three separate issues the individual
# suites would otherwise file.
#
# This script does not reimplement any suite's own evidence-gathering (that
# logic is frozen — see swarm-e2e-suite-lockdown). It only orchestrates:
# each wrapped suite still writes its own local report/evidence bundle
# exactly as it does when run standalone; this script captures each suite's
# full console output alongside those, and folds pointers/content into one
# consolidated summary.
#
# Every wrapped suite always runs with its own issue-filing suppressed
# (--no-issue) regardless of this script's own --github-issue flag — this
# script is the one that decides whether to file, once, after seeing every
# suite's result.
#
# Usage:
#   ./scripts/full_uat_suite.sh                      # run backend + E2E + UAT, local-only report
#   ./scripts/full_uat_suite.sh --github-issue        # same, plus file one consolidated issue if any suite failed
#   ./scripts/full_uat_suite.sh --skip-backend        # skip media_server_uat_tests.sh
#   ./scripts/full_uat_suite.sh --skip-e2e            # skip tv_e2e_suite.sh
#   ./scripts/full_uat_suite.sh --skip-uat            # skip tv_uat_suite.sh
#   ./scripts/full_uat_suite.sh --include-resilience  # also run the opt-in disruptive resilience suite
#   ./scripts/full_uat_suite.sh --device 192.168.0.148  # forwarded to both hardware suites
#   ./scripts/full_uat_suite.sh --all                   # forwarded to both hardware suites: force full fan-out
#
# Env vars (same defaults as the suites this wraps):
#   SWARM_GITHUB_REPOSITORY  where the consolidated issue is filed (default DotNetRockStar/swarm)
#   SWARM_E2E_ISSUE_LABEL    label applied to it (default "Testing")
#   SWARM_RUN_DIR            shared local run directory (default .run)
#
# Exit code: 0 only if every suite that ran reported zero failures; 1 if any
# suite reported a real failure; 2 for a bad argument. Safe to chain in CI
# or a script, same contract as the suites it wraps.

set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

GITHUB_REPOSITORY="${SWARM_GITHUB_REPOSITORY:-DotNetRockStar/swarm}"
ISSUE_LABEL="${SWARM_E2E_ISSUE_LABEL:-Testing}"
RUN_DIR="${SWARM_RUN_DIR:-.run}"

RUN_BACKEND=1
RUN_E2E=1
RUN_UAT=1
RUN_RESILIENCE=0
FILE_ISSUE=0
DEVICE_ARGS=()

usage() {
    sed -n '2,33p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --skip-backend) RUN_BACKEND=0; shift ;;
        --skip-e2e) RUN_E2E=0; shift ;;
        --skip-uat) RUN_UAT=0; shift ;;
        --include-resilience) RUN_RESILIENCE=1; shift ;;
        --github-issue) FILE_ISSUE=1; shift ;;
        --no-issue) FILE_ISSUE=0; shift ;;
        --device)
            [ $# -ge 2 ] || { echo "--device requires a value" >&2; exit 2; }
            DEVICE_ARGS=(--device "$2"); shift 2 ;;
        --all) DEVICE_ARGS=(--all); shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

RUN_STAMP="$(date +%Y%m%d-%H%M%S)"
REPORT_DIR="$RUN_DIR/full-uat-reports/$RUN_STAMP"
mkdir -p "$REPORT_DIR"
COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"

TOTAL_PASS=0
TOTAL_FAIL=0
TOTAL_SKIP=0
SUITE_ROWS_FILE="$REPORT_DIR/suite_rows.tsv"
: > "$SUITE_ROWS_FILE"

# Extracts "pass fail skip" from a captured `PASS: N   FAIL: N   SKIP: N`
# summary line (tv_e2e_suite.sh / tv_uat_suite.sh's own format). Falls back
# to "0 1 0" when the suite's own exit status was non-zero but no such line
# was ever printed (a hard crash/precondition failure before it could build
# its results table) so a suite that errored out early is never silently
# counted as a clean pass.
parse_pass_fail_skip_line() {
    local logfile="$1" exit_status="$2"
    local line
    line="$(grep -oE 'PASS: [0-9]+ +FAIL: [0-9]+ +SKIP: [0-9]+' "$logfile" 2>/dev/null | tail -1)"
    if [ -n "$line" ]; then
        echo "$line" | sed -E 's/PASS: ([0-9]+) +FAIL: ([0-9]+) +SKIP: ([0-9]+)/\1 \2 \3/'
    elif [ "$exit_status" -eq 2 ]; then
        # A precondition failure (server not running, no data dir, bad args)
        # never reached its results table — not a test failure, just an
        # environment problem the summary should still surface plainly.
        echo "0 0 0"
    elif [ "$exit_status" -eq 0 ]; then
        echo "0 0 0"
    else
        echo "0 1 0"
    fi
}

# Extracts "pass fail skip" from a `cargo test` results line
# (media_server_uat_tests.sh's underlying format), mapping "ignored" to
# skip. Same non-zero-with-no-line fallback as above.
parse_cargo_test_line() {
    local logfile="$1" exit_status="$2"
    local line
    line="$(grep -oE '[0-9]+ passed; [0-9]+ failed;.*[0-9]+ ignored' "$logfile" 2>/dev/null | tail -1)"
    if [ -n "$line" ]; then
        local pass fail skip
        pass="$(echo "$line" | grep -oE '^[0-9]+')"
        fail="$(echo "$line" | grep -oE '[0-9]+ failed' | grep -oE '^[0-9]+')"
        skip="$(echo "$line" | grep -oE '[0-9]+ ignored' | grep -oE '^[0-9]+')"
        echo "$pass $fail $skip"
    elif [ "$exit_status" -eq 0 ]; then
        echo "0 0 0"
    else
        echo "0 1 0"
    fi
}

# Runs one suite, capturing its full console output to its own log file
# under this run's report dir (that capture *is* this script's evidence
# contribution — each suite's own evidence bundle, e.g. tv_uat_suite.sh's
# per-FAIL TV-to-server dump, is untouched and still written to its usual
# place, which the parsed "Report written to" line below points back to).
run_suite() {
    local key="$1" label="$2" logfile="$3"; shift 3
    echo
    echo "=================================================================="
    echo "=== $label"
    echo "=================================================================="
    "$@" > "$logfile" 2>&1
    local status=$?
    tail -n 40 "$logfile"
    echo "--- ($label exit status: $status; full output: $logfile) ---"

    local own_report=""
    own_report="$(grep -oE '^==> Report written to .*$' "$logfile" 2>/dev/null | tail -1 | sed 's/^==> Report written to //')"

    local counts
    if [ "$key" = "backend" ]; then
        counts="$(parse_cargo_test_line "$logfile" "$status")"
    else
        counts="$(parse_pass_fail_skip_line "$logfile" "$status")"
    fi
    local pass fail skip
    read -r pass fail skip <<< "$counts"
    TOTAL_PASS=$((TOTAL_PASS + pass))
    TOTAL_FAIL=$((TOTAL_FAIL + fail))
    TOTAL_SKIP=$((TOTAL_SKIP + skip))

    local result="PASS"
    [ "$fail" -gt 0 ] && result="FAIL"

    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$key" "$label" "$result" "$pass" "$fail" "$skip" "$logfile" "$own_report" \
        >> "$SUITE_ROWS_FILE"
}

if [ "$RUN_BACKEND" -eq 1 ]; then
    run_suite "backend" "media_server_uat_tests.sh (backend UAT — no hardware)" \
        "$REPORT_DIR/media_server_uat_tests.log" \
        ./scripts/media_server_uat_tests.sh
fi

if [ "$RUN_E2E" -eq 1 ]; then
    run_suite "e2e" "tv_e2e_suite.sh (smoke test — real Fire TV)" \
        "$REPORT_DIR/tv_e2e_suite.log" \
        ./scripts/tv_e2e_suite.sh --no-issue "${DEVICE_ARGS[@]}"
fi

if [ "$RUN_UAT" -eq 1 ]; then
    run_suite "uat" "tv_uat_suite.sh (full UAT — real Fire TV)" \
        "$REPORT_DIR/tv_uat_suite.log" \
        ./scripts/tv_uat_suite.sh --no-issue "${DEVICE_ARGS[@]}"
fi

if [ "$RUN_RESILIENCE" -eq 1 ]; then
    run_suite "resilience" "tv_uat_resilience_suite.sh (opt-in — real Fire TV)" \
        "$REPORT_DIR/tv_uat_resilience_suite.log" \
        ./scripts/tv_uat_resilience_suite.sh "${DEVICE_ARGS[@]}"
fi

# --- Compile the consolidated Markdown summary ---
{
    echo "# Full SWARM UAT run — $RUN_STAMP"
    echo
    echo "Commit \`$COMMIT\`. Orchestrated by \`scripts/full_uat_suite.sh\`; each suite below also wrote its own local report/evidence exactly as it would running standalone."
    echo
    echo "**TOTAL — PASS: $TOTAL_PASS   FAIL: $TOTAL_FAIL   SKIP: $TOTAL_SKIP**"
    echo
    echo "| Suite | Result | Passed | Failed | Skipped |"
    echo "|---|---|---|---|---|"
    while IFS=$'\t' read -r key label result pass fail skip logfile own_report; do
        echo "| $label | $result | $pass | $fail | $skip |"
    done < "$SUITE_ROWS_FILE"
    echo
    while IFS=$'\t' read -r key label result pass fail skip logfile own_report; do
        echo "<details><summary><strong>$label — $result</strong></summary>"
        echo
        if [ -n "$own_report" ] && [ -f "$own_report" ]; then
            echo "Full per-test-case report (from \`$own_report\`):"
            echo
            cat "$own_report"
        else
            echo "Captured console output (from \`$logfile\`):"
            echo
            echo '```'
            cat "$logfile"
            echo '```'
        fi
        echo
        echo "</details>"
        echo
    done < "$SUITE_ROWS_FILE"
    echo "---"
    echo "_Every suite above is frozen by policy — see the \`swarm-e2e-suite-lockdown\` skill. An AI agent picking up a follow-up on this issue should fix the underlying product bug behind any FAIL above, not edit a suite's test logic to make it pass._"
} > "$REPORT_DIR/summary.md"

echo
echo "=================================================================="
cat "$REPORT_DIR/summary.md" | head -n 20
echo "... (full summary, including every suite's own report, in $REPORT_DIR/summary.md)"
echo "=================================================================="
echo "==> Summary written to $REPORT_DIR/summary.md"

if [ "$FILE_ISSUE" -eq 1 ] && [ "$TOTAL_FAIL" -gt 0 ] && command -v gh >/dev/null 2>&1; then
    TITLE="Full SWARM UAT run: $TOTAL_PASS passed, $TOTAL_FAIL failed, $TOTAL_SKIP skipped ($RUN_STAMP)"
    if ISSUE_URL="$(gh issue create --repo "$GITHUB_REPOSITORY" --title "$TITLE" --body-file "$REPORT_DIR/summary.md" --label "$ISSUE_LABEL" 2>&1)"; then
        echo "==> Findings posted: $ISSUE_URL"
    else
        echo "Could not post findings to GitHub (summary is still saved locally): $ISSUE_URL" >&2
    fi
elif [ "$FILE_ISSUE" -eq 1 ] && [ "$TOTAL_FAIL" -eq 0 ]; then
    echo "==> No failures — nothing to file. GitHub issues are only opened when TOTAL_FAIL > 0."
elif [ "$TOTAL_FAIL" -gt 0 ]; then
    echo "==> $TOTAL_FAIL failure(s) found. Pass --github-issue to file a consolidated issue for this run."
fi

[ "$TOTAL_FAIL" -eq 0 ]
