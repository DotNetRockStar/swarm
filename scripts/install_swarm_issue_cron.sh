#!/usr/bin/env bash

# Runs the issue worker in the foreground at a fixed interval. Any legacy
# crontab block installed by an older version of this script is removed first.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WORKER_PATH="$SCRIPT_DIR/swarm_issue_worker.sh"
USER_HOME_DIR="${HOME:?HOME must be set}"
STATE_DIR="${SWARM_ISSUE_WORKER_STATE_DIR:-$USER_HOME_DIR/.local/state/swarm-issue-worker}"
LOG_PATH="$STATE_DIR/cron.log"
INTERVAL_SECONDS="${SWARM_ISSUE_WORKER_INTERVAL_SECONDS:-600}"
CRONTAB_BIN="${CRONTAB_BIN:-$(command -v crontab || true)}"
BEGIN_MARKER="# BEGIN SWARM ISSUE WORKER"
END_MARKER="# END SWARM ISSUE WORKER"

log() {
    printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S%z')" "$*" | tee -a "$LOG_PATH"
}

remove_legacy_cron() {
    local current_crontab

    if [ -z "$CRONTAB_BIN" ] || [ ! -x "$CRONTAB_BIN" ]; then
        return 0
    fi

    current_crontab="$($CRONTAB_BIN -l 2>/dev/null || true)"
    if ! printf '%s\n' "$current_crontab" | grep -Fqx -- "$BEGIN_MARKER"; then
        return 0
    fi

    printf '%s\n' "$current_crontab" \
        | sed "/^${BEGIN_MARKER}$/,/^${END_MARKER}$/d" \
        | "$CRONTAB_BIN" -
    log "Removed the legacy SWARM issue worker crontab entry."
}

usage() {
    printf 'Usage: %s [--remove]\n' "$0" >&2
}

if [ "${1:-}" = "--remove" ]; then
    if [ "$#" -ne 1 ]; then
        usage
        exit 2
    fi
    mkdir -p -- "$STATE_DIR"
    remove_legacy_cron
    log "The SWARM issue worker is not running from this terminal."
    exit 0
fi
if [ "$#" -ne 0 ]; then
    usage
    exit 2
fi
if [ ! -x "$WORKER_PATH" ]; then
    printf 'Worker is not executable: %s\n' "$WORKER_PATH" >&2
    exit 1
fi
if ! [[ "$INTERVAL_SECONDS" =~ ^[1-9][0-9]*$ ]]; then
    printf 'SWARM_ISSUE_WORKER_INTERVAL_SECONDS must be a positive integer.\n' >&2
    exit 2
fi

mkdir -p -- "$STATE_DIR"
remove_legacy_cron

on_interrupt() {
    log "Ctrl+C received; stopped the SWARM issue worker."
    exit 130
}
trap on_interrupt INT TERM

if [ ! -t 0 ]; then
    printf 'Run this script in a terminal so it can securely prompt for the SMTP password.\n' >&2
    exit 1
fi
printf 'SMTP password (input is hidden): ' >&2
IFS= read -r -s SWARM_SMTP_PASSWORD
printf '\n' >&2
if [ -z "$SWARM_SMTP_PASSWORD" ]; then
    printf 'An SMTP password is required.\n' >&2
    exit 1
fi
export SWARM_SMTP_PASSWORD

log "Running the SWARM issue worker in this terminal every $INTERVAL_SECONDS seconds."
log "Live output is also appended to $LOG_PATH. Press Ctrl+C to stop."

while true; do
    log "Starting a worker run."
    set +e
    "$WORKER_PATH" 2>&1 | tee -a "$LOG_PATH"
    worker_status="${PIPESTATUS[0]}"
    set -e

    if [ "$worker_status" -ne 0 ]; then
        log "Worker exited with status $worker_status; it will retry after $INTERVAL_SECONDS seconds."
    else
        log "Worker run finished; next run starts in $INTERVAL_SECONDS seconds."
    fi
    sleep "$INTERVAL_SECONDS"
done
