#!/usr/bin/env python3
"""Run the SWARM issue worker continuously in the foreground.

The historical filename says "cron", but this is a foreground scheduler. It
also removes the legacy crontab block installed by older SWARM versions.
"""

from __future__ import annotations

import argparse
import atexit
import datetime as dt
import getpass
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Sequence


ISSUE_COMPLETED_EXIT_CODE = 10
QUOTA_PAUSED_EXIT_CODE = 11
BEGIN_MARKER = "# BEGIN SWARM ISSUE WORKER"
END_MARKER = "# END SWARM ISSUE WORKER"
WEEKDAY_NAMES = ("mon", "tue", "wed", "thu", "fri", "sat", "sun")


def timestamp() -> str:
    return dt.datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S%z")


def env_value(name: str, fallback: str) -> str:
    return os.environ.get(name, fallback)


def positive_integer(value: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def schedule_time(value: str) -> tuple[int, int]:
    try:
        parsed = dt.datetime.strptime(value, "%H:%M")
    except ValueError as error:
        raise argparse.ArgumentTypeError("must use 24-hour HH:MM format") from error
    return parsed.hour, parsed.minute


def schedule_days(value: str) -> frozenset[int]:
    names = [part.strip().lower()[:3] for part in value.split(",") if part.strip()]
    invalid = [name for name in names if name not in WEEKDAY_NAMES]
    if invalid or not names:
        raise argparse.ArgumentTypeError("must be a comma-separated list such as mon,tue,wed")
    return frozenset(WEEKDAY_NAMES.index(name) for name in names)


class Runner:
    def __init__(self, args: argparse.Namespace, worker_arguments: Sequence[str]) -> None:
        self.args = args
        self.worker_arguments = list(worker_arguments)
        self.script_dir = Path(__file__).resolve().parent
        self.state_dir = Path(args.state_dir).expanduser().resolve()
        self.log_path = (
            Path(args.log_path).expanduser().resolve()
            if args.log_path
            else self.state_dir / "cron.log"
        )
        self.lock_dir = self.state_dir / "runner.lock"
        self.worker = Path(args.worker).expanduser().resolve()
        self.snapshot = self.state_dir / "swarm_issue_worker.snapshot.py"
        self.in_progress_file = self.state_dir / "in-progress-issue.json"
        self.acquired_lock = False
        self.stop_requested = False
        self.log_path.parent.mkdir(parents=True, exist_ok=True)

    def log(self, message: str) -> None:
        line = f"[{timestamp()}] {message}"
        print(line, flush=True)
        self.state_dir.mkdir(parents=True, exist_ok=True)
        with self.log_path.open("a", encoding="utf-8") as stream:
            stream.write(line + "\n")

    def remove_legacy_cron(self) -> None:
        if not self.args.crontab_bin:
            return
        current = subprocess.run(
            [self.args.crontab_bin, "-l"], text=True, capture_output=True, check=False
        ).stdout
        if BEGIN_MARKER not in current.splitlines():
            return
        filtered: list[str] = []
        removing = False
        for line in current.splitlines():
            if line == BEGIN_MARKER:
                removing = True
                continue
            if line == END_MARKER:
                removing = False
                continue
            if not removing:
                filtered.append(line)
        payload = "\n".join(filtered) + ("\n" if filtered else "")
        result = subprocess.run(
            [self.args.crontab_bin, "-"], input=payload, text=True, capture_output=True, check=False
        )
        if result.returncode != 0:
            raise RuntimeError(f"Could not remove legacy crontab block: {result.stderr.strip()}")
        self.log("Removed the legacy SWARM issue worker crontab entry.")

    def acquire_lock(self) -> bool:
        self.state_dir.mkdir(parents=True, exist_ok=True)
        try:
            self.lock_dir.mkdir()
        except FileExistsError:
            pid_path = self.lock_dir / "pid"
            try:
                owner = int(pid_path.read_text().strip())
                os.kill(owner, 0)
            except (OSError, ValueError):
                pid_path.unlink(missing_ok=True)
                try:
                    self.lock_dir.rmdir()
                except OSError:
                    pass
                try:
                    self.lock_dir.mkdir()
                except FileExistsError:
                    self.log("Another foreground runner acquired the lock during stale-lock recovery; exiting.")
                    return False
            else:
                self.log(f"Another foreground SWARM issue runner is already active as pid {owner}; exiting.")
                return False
        (self.lock_dir / "pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
        self.acquired_lock = True
        return True

    def release_lock(self) -> None:
        if not self.acquired_lock:
            return
        (self.lock_dir / "pid").unlink(missing_ok=True)
        try:
            self.lock_dir.rmdir()
        except OSError:
            pass
        self.acquired_lock = False

    def transcode_active(self) -> bool:
        if not self.args.pgrep_bin:
            return False
        result = subprocess.run(
            [
                self.args.pgrep_bin,
                "-f",
                self.args.transcode_pattern,
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        return result.returncode == 0

    def prune_cargo_target(self) -> None:
        target = (
            Path(self.args.cargo_target_dir).expanduser().resolve()
            if self.args.cargo_target_dir
            else Path(self.args.repo_dir).expanduser().resolve() / "target"
        )
        if not target.is_dir():
            return
        size = sum(path.stat().st_size for path in target.rglob("*") if path.is_file())
        limit = self.args.cargo_target_max_gib * 1024**3
        if size <= limit:
            return
        for process in ("cargo", "rustc"):
            if self.args.pgrep_bin and subprocess.run(
                [self.args.pgrep_bin, "-x", process],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            ).returncode == 0:
                self.log(
                    f"Cargo target exceeds {self.args.cargo_target_max_gib} GiB, but a Rust build is active; cleanup deferred."
                )
                return
        if not self.args.cargo_bin:
            self.log(
                f"Cargo target exceeds {self.args.cargo_target_max_gib} GiB, but cargo is unavailable; cleanup deferred."
            )
            return
        self.log(
            f"Cargo target exceeds {self.args.cargo_target_max_gib} GiB; removing generated build artifacts."
        )
        result = subprocess.run([self.args.cargo_bin, "clean"], cwd=self.args.repo_dir, check=False)
        self.log(
            "Cargo build-artifact cleanup completed."
            if result.returncode == 0
            else "Warning: Cargo build-artifact cleanup failed; it will be retried later."
        )

    def git(self, *arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [self.args.git_bin, "-C", self.args.repo_dir, *arguments],
            text=True,
            capture_output=True,
            check=False,
        )

    def synchronize_repository(self) -> bool:
        if not self.args.git_bin:
            self.log("Git is unavailable; deferring the worker until the repository can be synchronized.")
            return False
        if self.git("rev-parse", "--is-inside-work-tree").returncode != 0:
            self.log(f"Worker repository is not a Git checkout: {self.args.repo_dir}; deferring this run.")
            return False
        branch = self.git("branch", "--show-current").stdout.strip()
        if self.in_progress_file.exists():
            self.log(
                f"Saved issue work owns {branch or 'the current checkout'}; repository synchronization "
                "will occur after the worker returns to main."
            )
            return True
        if self.git("status", "--porcelain").stdout.strip():
            self.log("Repository has uncommitted work with no saved issue owner; deferring synchronization and AI.")
            return False
        if branch != self.args.base_branch:
            switched = self.git("switch", self.args.base_branch)
            if switched.returncode != 0:
                detail = switched.stderr.strip() or switched.stdout.strip() or "git switch failed"
                self.log(f"Could not return to {self.args.base_branch}: {detail}; deferring this run.")
                return False
            self.log(f"Returned the idle checkout to {self.args.base_branch} before synchronization.")
        before = self.git("rev-parse", "HEAD").stdout.strip()
        pulled = self.git("pull", "--ff-only", self.args.remote_name, self.args.base_branch)
        if pulled.returncode != 0:
            detail = pulled.stderr.strip() or pulled.stdout.strip() or "git pull failed"
            self.log(
                f"Could not fast-forward {self.args.base_branch} from {self.args.remote_name}: "
                f"{detail}; deferring this run."
            )
            return False
        after = self.git("rev-parse", "HEAD").stdout.strip()
        if after != before:
            self.log(
                f"Fast-forwarded {self.args.base_branch} from {self.args.remote_name} "
                f"({before[:8]} -> {after[:8]}); the next worker snapshot uses the updated code."
            )
        else:
            self.log(f"Local {self.args.base_branch} is synchronized with {self.args.remote_name}.")
        return True

    def run_worker(self, smtp_password: str) -> int:
        shutil.copy2(self.worker, self.snapshot)
        environment = os.environ.copy()
        environment["SWARM_SMTP_PASSWORD"] = smtp_password
        environment["SWARM_REPO_DIR"] = str(Path(self.args.repo_dir).expanduser().resolve())
        environment["SWARM_ISSUE_WORKER_STATE_DIR"] = str(self.state_dir)
        environment["SWARM_ISSUE_WORKER_SCRIPT_DIR"] = str(self.script_dir)
        environment["GIT_BIN"] = self.args.git_bin
        environment["SWARM_BASE_BRANCH"] = self.args.base_branch
        environment["SWARM_GIT_REMOTE"] = self.args.remote_name
        environment["PYTHONPATH"] = os.pathsep.join(
            [str(self.script_dir), environment.get("PYTHONPATH", "")]
        ).rstrip(os.pathsep)
        command = [self.args.python_bin, str(self.snapshot), *self.worker_arguments]
        process = subprocess.Popen(
            command,
            cwd=self.args.repo_dir,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        assert process.stdout
        with process.stdout, self.log_path.open("a", encoding="utf-8") as log_stream:
            for line in process.stdout:
                print(line, end="", flush=True)
                log_stream.write(line)
                log_stream.flush()
        return process.wait()

    def sleep(self) -> None:
        deadline = time.monotonic() + self.args.interval_seconds
        while not self.stop_requested and time.monotonic() < deadline:
            time.sleep(min(1, deadline - time.monotonic()))

    def scheduled_days(self) -> frozenset[int]:
        if self.args.schedule_mode == "weekdays":
            return frozenset(range(5))
        if self.args.schedule_mode == "custom":
            return self.args.schedule_days
        return frozenset(range(7))

    def next_scheduled_run(self, now: dt.datetime | None = None) -> dt.datetime:
        current = now or dt.datetime.now().astimezone()
        hour, minute = self.args.schedule_time
        allowed_days = self.scheduled_days()
        for offset in range(8):
            day = current.date() + dt.timedelta(days=offset)
            if day.weekday() not in allowed_days:
                continue
            # Calling astimezone() on a naive local datetime lets the host OS
            # apply the correct UTC offset even when the next run crosses a
            # daylight-saving boundary.
            candidate = dt.datetime.combine(day, dt.time(hour, minute)).astimezone()
            if candidate > current:
                return candidate
        raise RuntimeError("Could not determine the next scheduled worker run")

    def wait_for_schedule(self) -> bool:
        target = self.next_scheduled_run()
        self.log(f"Next scheduled issue-worker check: {target:%A, %Y-%m-%d at %H:%M %Z}.")
        while not self.stop_requested:
            remaining = (target - dt.datetime.now().astimezone()).total_seconds()
            if remaining <= 0:
                return True
            time.sleep(min(1, remaining))
        return False

    def run(self) -> int:
        self.state_dir.mkdir(parents=True, exist_ok=True)
        self.remove_legacy_cron()
        if self.args.remove:
            self.log("The SWARM issue worker is not running from this terminal.")
            return 0
        if self.args.check_transcode_active:
            return 0 if self.transcode_active() else 1
        if not self.worker.is_file():
            raise RuntimeError(f"Worker was not found: {self.worker}")
        if not self.acquire_lock():
            return 0
        atexit.register(self.release_lock)

        def stop(_signum: int, _frame: object) -> None:
            self.stop_requested = True

        signal.signal(signal.SIGINT, stop)
        signal.signal(signal.SIGTERM, stop)
        smtp_password = os.environ.pop("SWARM_SMTP_PASSWORD", "")
        if not self.args.no_email and not smtp_password:
            if not sys.stdin.isatty():
                raise RuntimeError("Run in a terminal so the SMTP password can be entered securely, or use --no-email")
            smtp_password = getpass.getpass("SMTP password: ")
        if not self.args.no_email and not smtp_password:
            raise RuntimeError("An SMTP password is required unless --no-email is used")
        if self.args.no_email and "--no-email" not in self.worker_arguments:
            self.worker_arguments.append("--no-email")

        self.log(
            "Running the SWARM issue worker in this terminal. Queued issues run back to back; "
            + (
                f"idle checks occur every {self.args.interval_seconds} seconds."
                if self.args.schedule_mode == "continuous"
                else f"queue checks use the {self.args.schedule_mode} schedule."
            )
        )
        self.log(f"Live output is also appended to {self.log_path}. Press Ctrl+C to stop.")
        scheduled_tick_active = self.args.once or self.args.schedule_mode == "continuous"
        try:
            while not self.stop_requested:
                if not scheduled_tick_active:
                    if not self.wait_for_schedule():
                        break
                    scheduled_tick_active = True
                self.log("Starting a worker run.")
                if self.transcode_active():
                    self.log(
                        f"A SWARM media transcode is active; deferring AI and build work for "
                        f"{self.args.interval_seconds} seconds."
                    )
                    if self.args.once:
                        return 0
                    self.sleep()
                    continue
                if not self.synchronize_repository():
                    if self.args.once:
                        return 0
                    self.sleep()
                    continue
                status = self.run_worker(smtp_password)
                self.prune_cargo_target()
                if status == ISSUE_COMPLETED_EXIT_CODE:
                    self.log("Issue completed successfully; checking the queue again immediately.")
                    if self.args.once:
                        return status
                    continue
                if status == QUOTA_PAUSED_EXIT_CODE:
                    self.log(
                        "The active AI session was safely shelved for usage; checking immediately for another ready issue."
                    )
                    if self.args.once:
                        return status
                    continue
                if status:
                    self.log(
                        f"Worker exited with status {status}; it will retry after {self.args.interval_seconds} seconds."
                    )
                else:
                    self.log(
                        f"No issue can be worked now; checking again in {self.args.interval_seconds} seconds."
                    )
                if self.args.once:
                    return status
                if self.args.schedule_mode == "continuous":
                    self.sleep()
                else:
                    # A scheduled tick drains completed/quota-shelved issues
                    # immediately via the branches above. Once the worker is
                    # idle (or errored), return to the next configured slot.
                    scheduled_tick_active = False
        finally:
            self.release_lock()
        self.log("Ctrl+C received; stopped the SWARM issue worker.")
        return 130 if self.stop_requested else 0


def build_parser() -> argparse.ArgumentParser:
    script_dir = Path(__file__).resolve().parent
    home = Path.home()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--remove", action="store_true")
    parser.add_argument("--check-transcode-active", action="store_true")
    parser.add_argument("--once", action="store_true")
    parser.add_argument("--no-email", action="store_true")
    parser.add_argument(
        "--interval-seconds",
        type=positive_integer,
        default=positive_integer(env_value("SWARM_ISSUE_WORKER_INTERVAL_SECONDS", "600")),
    )
    parser.add_argument(
        "--schedule-mode",
        choices=("continuous", "daily", "weekdays", "custom"),
        default=env_value("SWARM_ISSUE_WORKER_SCHEDULE_MODE", "continuous"),
        help="continuous polling, daily, weekdays, or selected custom days",
    )
    parser.add_argument(
        "--schedule-time",
        type=schedule_time,
        default=schedule_time(env_value("SWARM_ISSUE_WORKER_SCHEDULE_TIME", "09:00")),
        metavar="HH:MM",
        help="local start time for daily/weekday/custom schedules",
    )
    parser.add_argument(
        "--schedule-days",
        type=schedule_days,
        default=schedule_days(env_value("SWARM_ISSUE_WORKER_SCHEDULE_DAYS", "mon,tue,wed,thu,fri")),
        metavar="DAYS",
        help="comma-separated weekdays used by --schedule-mode custom",
    )
    parser.add_argument(
        "--cargo-target-max-gib",
        type=positive_integer,
        default=positive_integer(env_value("SWARM_CARGO_TARGET_MAX_GIB", "5")),
    )
    parser.add_argument("--repo-dir", default=env_value("SWARM_REPO_DIR", str(script_dir.parent.parent)))
    parser.add_argument(
        "--state-dir",
        default=env_value("SWARM_ISSUE_WORKER_STATE_DIR", str(home / ".local/state/swarm-issue-worker")),
    )
    parser.add_argument(
        "--worker", default=env_value("SWARM_ISSUE_WORKER_PATH", str(script_dir / "swarm_issue_worker.py"))
    )
    parser.add_argument("--python-bin", default=env_value("PYTHON_BIN", shutil.which("python3") or "python3"))
    parser.add_argument("--crontab-bin", default=env_value("CRONTAB_BIN", shutil.which("crontab") or ""))
    parser.add_argument("--pgrep-bin", default=env_value("PGREP_BIN", shutil.which("pgrep") or ""))
    parser.add_argument("--cargo-bin", default=env_value("CARGO_BIN", shutil.which("cargo") or ""))
    parser.add_argument("--git-bin", default=env_value("GIT_BIN", shutil.which("git") or ""))
    parser.add_argument("--base-branch", default=env_value("SWARM_BASE_BRANCH", "main"))
    parser.add_argument("--remote-name", default=env_value("SWARM_GIT_REMOTE", "origin"))
    parser.add_argument("--log-path", default=env_value("SWARM_ISSUE_WORKER_LOG_PATH", ""))
    parser.add_argument("--cargo-target-dir", default=env_value("SWARM_CARGO_TARGET_DIR", ""))
    parser.add_argument(
        "--transcode-pattern",
        default=env_value(
            "SWARM_TRANSCODE_PROCESS_PATTERN",
            r"[f]fmpeg .* -f hls .*app[.]swarm[.]server/transcodes/",
        ),
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args, worker_arguments = parser.parse_known_args(argv)
    if args.remove and args.check_transcode_active:
        parser.error("--remove and --check-transcode-active are mutually exclusive")
    return Runner(args, worker_arguments).run()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1)
