#!/usr/bin/env bash

# Runs the issue worker in the foreground at a fixed interval. Any legacy
# crontab block installed by an older version of this script is removed first.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WORKER_PATH="$SCRIPT_DIR/swarm_issue_worker.sh"
USER_HOME_DIR="${HOME:?HOME must be set}"
STATE_DIR="${SWARM_ISSUE_WORKER_STATE_DIR:-$USER_HOME_DIR/.local/state/swarm-issue-worker}"
LOG_PATH="$STATE_DIR/cron.log"
WORKER_SNAPSHOT_PATH="$STATE_DIR/swarm_issue_worker.snapshot.sh"
RUNNER_LOCK_DIR="$STATE_DIR/runner.lock"
INTERVAL_SECONDS="${SWARM_ISSUE_WORKER_INTERVAL_SECONDS:-600}"
CARGO_TARGET_MAX_GIB="${SWARM_CARGO_TARGET_MAX_GIB:-1}"
ISSUE_COMPLETED_EXIT_CODE=10
QUOTA_PAUSED_EXIT_CODE=11
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
if ! [[ "$CARGO_TARGET_MAX_GIB" =~ ^[1-9][0-9]*$ ]]; then
    printf 'SWARM_CARGO_TARGET_MAX_GIB must be a positive integer.\n' >&2
    exit 2
fi

prune_cargo_target_if_needed() {
    local target_dir="$SCRIPT_DIR/../target"
    local target_kib
    local maximum_kib=$((CARGO_TARGET_MAX_GIB * 1024 * 1024))

    [ -d "$target_dir" ] || return 0
    target_kib="$(du -sk "$target_dir" 2>/dev/null | awk '{ print $1 }')"
    [[ "$target_kib" =~ ^[0-9]+$ ]] || return 0
    [ "$target_kib" -gt "$maximum_kib" ] || return 0

    # Never remove artifacts from underneath a manual or unattended build.
    # A later idle worker cycle will retry the size check.
    if pgrep -x cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1; then
        log "Cargo target exceeds ${CARGO_TARGET_MAX_GIB} GiB, but a Rust build is active; cleanup deferred."
        return 0
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        log "Cargo target exceeds ${CARGO_TARGET_MAX_GIB} GiB, but cargo is unavailable; cleanup deferred."
        return 0
    fi

    log "Cargo target exceeds ${CARGO_TARGET_MAX_GIB} GiB; removing generated build artifacts."
    if (cd -- "$SCRIPT_DIR/.." && cargo clean); then
        log "Cargo build-artifact cleanup completed."
    else
        log "Warning: Cargo build-artifact cleanup failed; it will be retried later."
    fi
}

cleanup_runner_lock() {
    if [ -f "$RUNNER_LOCK_DIR/pid" ] \
        && [ "$(sed -n '1p' "$RUNNER_LOCK_DIR/pid" 2>/dev/null || true)" = "$$" ]; then
        rm -f -- "$RUNNER_LOCK_DIR/pid"
        rmdir -- "$RUNNER_LOCK_DIR" 2>/dev/null || true
    fi
}

acquire_runner_lock() {
    local owner_pid

    if mkdir -- "$RUNNER_LOCK_DIR" 2>/dev/null; then
        printf '%s\n' "$$" > "$RUNNER_LOCK_DIR/pid"
        return 0
    fi
    owner_pid="$(sed -n '1p' "$RUNNER_LOCK_DIR/pid" 2>/dev/null || true)"
    if [[ "$owner_pid" =~ ^[0-9]+$ ]] && kill -0 "$owner_pid" 2>/dev/null; then
        log "Another foreground SWARM issue runner is already active as pid $owner_pid; exiting."
        exit 0
    fi

    rm -f -- "$RUNNER_LOCK_DIR/pid" 2>/dev/null || true
    rmdir -- "$RUNNER_LOCK_DIR" 2>/dev/null || true
    if ! mkdir -- "$RUNNER_LOCK_DIR" 2>/dev/null; then
        log "Another foreground runner acquired the lock during stale-lock recovery; exiting."
        exit 0
    fi
    printf '%s\n' "$$" > "$RUNNER_LOCK_DIR/pid"
}

mkdir -p -- "$STATE_DIR"
remove_legacy_cron
acquire_runner_lock
trap cleanup_runner_lock EXIT

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

log "Running the SWARM issue worker in this terminal. Queued issues run back to back; idle checks occur every $INTERVAL_SECONDS seconds."
log "Live output is also appended to $LOG_PATH. Press Ctrl+C to stop."

while true; do
    log "Starting a worker run."
    # Run an immutable snapshot so editing or updating the repository while a
    # worker is active cannot change the script underneath Bash's read offset.
    /bin/cp "$WORKER_PATH" "$WORKER_SNAPSHOT_PATH"
    set +e
    SWARM_ISSUE_WORKER_SCRIPT_DIR="$SCRIPT_DIR" \
        /bin/bash "$WORKER_SNAPSHOT_PATH" 2>&1 | tee -a "$LOG_PATH"
    worker_status="${PIPESTATUS[0]}"
    set -e
    prune_cargo_target_if_needed

    if [ "$worker_status" -eq "$ISSUE_COMPLETED_EXIT_CODE" ]; then
        log "Issue completed successfully; checking the queue again immediately."
        continue
    elif [ "$worker_status" -eq "$QUOTA_PAUSED_EXIT_CODE" ]; then
        log "The active AI session was safely shelved for usage; checking immediately for another ready issue."
        continue
    elif [ "$worker_status" -ne 0 ]; then
        log "Worker exited with status $worker_status; it will retry after $INTERVAL_SECONDS seconds."
    else
        log "No issue can be worked now; checking again in $INTERVAL_SECONDS seconds."
    fi
    sleep "$INTERVAL_SECONDS"
done
