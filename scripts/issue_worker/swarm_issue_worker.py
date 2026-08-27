#!/usr/bin/env python3
"""Process at most one assigned GitHub issue with Claude or Codex.

This is the Python implementation of the SWARM unattended issue worker. The
state files and exit codes intentionally remain compatible with the former
shell worker so an upgrade can resume existing active and quota-paused runs.
"""

from __future__ import annotations

import argparse
import contextlib
import dataclasses
import datetime as dt
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path
from typing import Any, Iterable, Sequence

SCRIPT_HOME = Path(os.environ.get("SWARM_ISSUE_WORKER_SCRIPT_DIR", Path(__file__).resolve().parent)).resolve()
if str(SCRIPT_HOME) not in sys.path:
    sys.path.insert(0, str(SCRIPT_HOME))

from github_app_auth import DEFAULT_CONFIG_PATH, GitHubAppAuth


ISSUE_COMPLETED_EXIT_CODE = 10
QUOTA_PAUSED_EXIT_CODE = 11
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
QUOTA_RE = re.compile(
    r"usage limit|rate[ _-]?limit|quota|credits? (?:are )?(?:exhausted|unavailable)|"
    r"limit (?:has been )?reached|hit your .*limit|resets? at|insufficient_quota",
    re.IGNORECASE,
)
COMMIT_MARKER_RE = re.compile(r"swarm-issue-worker:commit:([0-9a-f]{40})")
THROUGH_COMMENT_RE = re.compile(r"through-comment:([0-9]+)")
PREVIOUS_AI_RE = re.compile(r"(?:Completed|Reworked) by \*\*(Claude|Codex)\*\*")
AUTOPILOT_INSTRUCTION = (
    "This is an unattended autopilot run. Do not ask the user questions, request confirmation, "
    "or pause for interactive input. Resolve ambiguity from the issue and repository, make "
    "reasonable safe assumptions, and implement the approach you recommend. If several valid "
    "approaches exist, choose the best maintainable option yourself. Only report a blocker when "
    "required credentials, authority, or external information are genuinely unavailable."
)
SUMMARY_INSTRUCTION = (
    "Your final response is shown in the terminal and posted to GitHub as rendered Markdown. "
    "Keep it concise and use exactly these headings: '## Summary', '## Changes', "
    "'## Verification', and '## Operational notes'. Under Summary, state the outcome and the "
    "problem resolved in one short paragraph. Under Changes and Verification, use short bullets. "
    "Under Operational notes, state whether the commit was pushed and mention only deployment, "
    "restart, migration, or follow-up requirements that actually apply; otherwise write '- None.' "
    "Do not include code snippets, diffs, file contents, command transcripts, or step-by-step "
    "implementation output."
)


class WorkerError(RuntimeError):
    pass


def timestamp() -> str:
    return dt.datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S%z")


def iso_timestamp() -> str:
    return dt.datetime.now().astimezone().isoformat(timespec="seconds")


def log(message: str) -> None:
    print(f"[{timestamp()}] {message}", file=sys.stderr, flush=True)


def env_value(name: str, fallback: str) -> str:
    return os.environ.get(name, fallback)


def env_bool(name: str, fallback: bool = False) -> bool:
    value = os.environ.get(name)
    if value is None:
        return fallback
    return value.lower() in {"1", "true", "yes", "on"}


def csv_values(value: str) -> tuple[str, ...]:
    return tuple(part.strip() for part in value.split(",") if part.strip())


def command_available(command: str | None) -> bool:
    return bool(command and (shutil.which(command) or Path(command).exists()))


def run_command(
    command: Sequence[str | Path],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    check: bool = True,
    capture: bool = True,
) -> subprocess.CompletedProcess[str]:
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    result = subprocess.run(
        [str(item) for item in command],
        cwd=cwd,
        env=merged_env,
        input=input_text,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        check=False,
    )
    if check and result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        raise WorkerError(f"Command failed ({' '.join(map(str, command))}): {detail}")
    return result


def atomic_write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    handle, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary_path = Path(temporary)
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as stream:
            json.dump(value, stream, indent=2, sort_keys=True)
            stream.write("\n")
        os.replace(temporary_path, path)
    finally:
        temporary_path.unlink(missing_ok=True)


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def flatten_pages(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        raise WorkerError("GitHub returned a non-list response")
    if value and all(isinstance(item, list) for item in value):
        return [entry for page in value for entry in page]
    return list(value)


@dataclasses.dataclass(frozen=True)
class Config:
    script_dir: Path
    repo_dir: Path
    state_dir: Path
    github_repository: str
    github_assignee: str
    trusted_followup_authors: tuple[str, ...]
    completion_authors: tuple[str, ...]
    ready_label: str
    minimum_remaining_percent: float
    claude_model: str
    codex_model: str
    claude_effort: str
    codex_effort: str
    preferred_provider: str
    email_to: str
    smtp_credentials_file: Path | None
    smtp_password: str
    no_email: bool
    dry_run: bool
    gh_bin: str
    git_bin: str
    python_bin: str
    claude_bin: str | None
    codex_bin: str | None
    github_apps_config: Path
    openssl_bin: str
    require_bot_auth: bool
    delivery_mode: str
    auto_approve: bool
    auto_merge: bool
    branch_prefix: str
    base_branch: str
    remote_name: str
    merge_method: str
    github_host: str

    @classmethod
    def from_args(cls, args: argparse.Namespace) -> "Config":
        script_dir = SCRIPT_HOME
        smtp_file = Path(args.smtp_credentials_file).expanduser() if args.smtp_credentials_file else None
        return cls(
            script_dir=script_dir,
            repo_dir=Path(args.repo_dir).expanduser().resolve(),
            state_dir=Path(args.state_dir).expanduser().resolve(),
            github_repository=args.github_repository,
            github_assignee=args.assignee,
            trusted_followup_authors=tuple(args.trusted_followup_author),
            completion_authors=tuple(args.completion_author),
            ready_label=args.ready_label,
            minimum_remaining_percent=args.minimum_remaining_percent,
            claude_model=args.claude_model,
            codex_model=args.codex_model,
            claude_effort=args.claude_effort,
            codex_effort=args.codex_effort,
            preferred_provider=args.preferred_provider,
            email_to=args.email_to,
            smtp_credentials_file=smtp_file,
            smtp_password=os.environ.pop("SWARM_SMTP_PASSWORD", ""),
            no_email=args.no_email,
            dry_run=args.dry_run,
            gh_bin=args.gh_bin,
            git_bin=args.git_bin,
            python_bin=args.python_bin,
            claude_bin=args.claude_bin or None,
            codex_bin=args.codex_bin or None,
            github_apps_config=Path(args.github_apps_config).expanduser(),
            openssl_bin=args.openssl_bin,
            require_bot_auth=args.require_bot_auth,
            delivery_mode=args.delivery_mode,
            auto_approve=args.auto_approve,
            auto_merge=args.auto_merge,
            branch_prefix=args.branch_prefix.strip("/"),
            base_branch=args.base_branch,
            remote_name=args.remote_name,
            merge_method=args.merge_method,
            github_host=args.github_host,
        )


class PidLock:
    def __init__(self, path: Path, label: str) -> None:
        self.path = path
        self.label = label
        self.acquired = False

    def __enter__(self) -> "PidLock":
        self.path.parent.mkdir(parents=True, exist_ok=True)
        try:
            self.path.mkdir()
        except FileExistsError:
            pid_path = self.path / "pid"
            try:
                owner = int(pid_path.read_text().strip())
                os.kill(owner, 0)
            except (OSError, ValueError):
                pid_path.unlink(missing_ok=True)
                with contextlib.suppress(OSError):
                    self.path.rmdir()
                try:
                    self.path.mkdir()
                except FileExistsError as error:
                    raise WorkerError(f"Another {self.label} acquired the lock during recovery") from error
            else:
                log(f"Another {self.label} is already running as pid {owner}; skipping this run.")
                raise SystemExit(0)
        (self.path / "pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
        self.acquired = True
        return self

    def __exit__(self, *_: object) -> None:
        if self.acquired:
            (self.path / "pid").unlink(missing_ok=True)
            with contextlib.suppress(OSError):
                self.path.rmdir()


class GitHubClient:
    def __init__(self, config: Config, apps: GitHubAppAuth) -> None:
        self.config = config
        self.apps = apps

    def environment(self, provider: str | None = None, *, require: bool | None = None) -> dict[str, str]:
        if provider and self.apps.configured(provider):
            return self.apps.bot_environment(provider)
        must_use_bot = self.config.require_bot_auth if require is None else require
        if provider and must_use_bot:
            raise WorkerError(f"GitHub App authentication is required but not configured for {provider}")
        return {}

    def gh(self, arguments: Sequence[str], provider: str | None = None, input_text: str | None = None) -> str:
        result = run_command(
            [self.config.gh_bin, *arguments],
            cwd=self.config.repo_dir,
            env=self.environment(provider),
            input_text=input_text,
        )
        return result.stdout

    def api_list(self, endpoint: str, fields: dict[str, str | int] | None = None) -> list[dict[str, Any]]:
        arguments = ["api", "--method", "GET", "--paginate", "--slurp", endpoint]
        for key, value in (fields or {}).items():
            flag = "-F" if isinstance(value, int) else "-f"
            arguments.extend([flag, f"{key}={value}"])
        return flatten_pages(json.loads(self.gh(arguments)))


@dataclasses.dataclass
class IssueContext:
    number: int
    title: str
    body: str
    labels: list[str]
    url: str
    work_type: str = "initial"
    previous_commit_sha: str = ""
    previous_ai: str = ""
    previous_completion_comment: dict[str, Any] | None = None
    followup_comments: list[dict[str, Any]] = dataclasses.field(default_factory=list)
    trigger_comment_id: int | None = None


@dataclasses.dataclass
class ProviderChoice:
    name: str
    model: str
    effort: str
    session_id: str = ""
    resume: bool = False

    @property
    def key(self) -> str:
        return self.name.lower()


def is_worker_comment(comment: dict[str, Any]) -> bool:
    return "<!-- swarm-issue-worker:" in str(comment.get("body") or "")


def extract_completion_metadata(
    comments: Iterable[dict[str, Any]], completion_authors: set[str]
) -> dict[str, Any] | None:
    matches = []
    for comment in sorted(comments, key=lambda item: int(item.get("id", 0))):
        body = str(comment.get("body") or "")
        author = str((comment.get("user") or {}).get("login") or "")
        marker = COMMIT_MARKER_RE.search(body)
        if marker and author in completion_authors:
            matches.append({"commit_sha": marker.group(1), "comment_id": int(comment["id"]), "author": author})
    return matches[-1] if matches else None


def extract_followup_metadata(
    comments: Iterable[dict[str, Any]],
    trusted_followup_authors: set[str],
    completion_authors: set[str],
) -> dict[str, Any] | None:
    ordered = sorted(comments, key=lambda item: int(item.get("id", 0)))
    completion_comments = []
    for comment in ordered:
        body = str(comment.get("body") or "")
        author = str((comment.get("user") or {}).get("login") or "")
        if COMMIT_MARKER_RE.search(body) and author in completion_authors:
            completion_comments.append(comment)
    if not completion_comments:
        return None
    completion = completion_comments[-1]
    body = str(completion.get("body") or "")
    commit_match = COMMIT_MARKER_RE.search(body)
    ai_match = PREVIOUS_AI_RE.search(body)
    through_match = THROUGH_COMMENT_RE.search(body)
    processed_through = int(through_match.group(1)) if through_match else int(completion["id"])
    followups = []
    for comment in ordered:
        author = str((comment.get("user") or {}).get("login") or "")
        if (
            int(comment.get("id", 0)) > processed_through
            and not is_worker_comment(comment)
            and author in trusted_followup_authors
        ):
            followups.append(
                {
                    "id": int(comment["id"]),
                    "author": author,
                    "created_at": str(comment.get("created_at") or ""),
                    "body": str(comment.get("body") or ""),
                }
            )
    if not followups:
        return None
    return {
        "previous_commit_sha": commit_match.group(1) if commit_match else "",
        "previous_ai": ai_match.group(1) if ai_match else "",
        "previous_completion_comment": {
            "id": int(completion["id"]),
            "author": str((completion.get("user") or {}).get("login") or "unknown"),
            "created_at": str(completion.get("created_at") or ""),
            "body": body,
        },
        "followup_comments": followups,
        "trigger_comment_id": int(followups[-1]["id"]),
        "trigger_created_at": str(followups[0]["created_at"]),
    }


class Worker:
    def __init__(self, config: Config) -> None:
        self.config = config
        self.state = config.state_dir
        self.state.mkdir(parents=True, exist_ok=True)
        self.lock_dir = self.state / "worker.lock"
        self.completed_file = self.state / "completed-issues"
        self.pending_file = self.state / "pending-email.json"
        self.in_progress_file = self.state / "in-progress-issue.json"
        self.paused_dir = self.state / "quota-paused-issues"
        self.closed_paused_dir = self.state / "closed-paused-issues"
        self.ai_output_file = self.state / "last-ai-output.log"
        self.ai_diagnostic_file = self.state / "last-ai-diagnostic.log"
        self.apps = GitHubAppAuth(config.github_apps_config, config.openssl_bin)
        self.github = GitHubClient(config, self.apps)
        self.choice: ProviderChoice | None = None
        self.issue: IssueContext | None = None
        self.quota_resume_ready = False

    def git(self, *arguments: str, env: dict[str, str] | None = None, check: bool = True) -> str:
        return run_command(
            [self.config.git_bin, "-C", self.config.repo_dir, *arguments], env=env, check=check
        ).stdout.strip()

    def git_ok(self, *arguments: str) -> bool:
        return run_command(
            [self.config.git_bin, "-C", self.config.repo_dir, *arguments], check=False
        ).returncode == 0

    def read_state(self, path: Path | None = None) -> dict[str, Any]:
        return read_json(path or self.in_progress_file)

    def write_state(self, value: dict[str, Any], path: Path | None = None) -> None:
        atomic_write_json(path or self.in_progress_file, value)

    def update_state(self, **changes: Any) -> dict[str, Any]:
        state = self.read_state()
        state.update(changes)
        self.write_state(state)
        return state

    def completed_numbers(self) -> set[int]:
        if not self.completed_file.exists():
            return set()
        return {
            int(line)
            for line in self.completed_file.read_text(encoding="utf-8").splitlines()
            if line.isdigit()
        }

    def record_completed(self, issue_number: int) -> None:
        completed = self.completed_numbers()
        if issue_number not in completed:
            with self.completed_file.open("a", encoding="utf-8") as stream:
                stream.write(f"{issue_number}\n")

    def clear_in_progress(self, issue_number: int) -> None:
        if self.in_progress_file.exists() and int(self.read_state().get("issue_number", -1)) == issue_number:
            self.in_progress_file.unlink()

    @property
    def trusted_followup_authors(self) -> set[str]:
        return set(self.config.trusted_followup_authors)

    @property
    def completion_authors(self) -> set[str]:
        return set(self.config.completion_authors) | self.apps.completion_authors()

    def comments(self, issue_number: int) -> list[dict[str, Any]]:
        return self.github.api_list(
            f"repos/{self.config.github_repository}/issues/{issue_number}/comments", {"per_page": 100}
        )

    def claude_capacity(self) -> int:
        if not command_available(self.config.claude_bin):
            log("Claude remaining quota — unavailable (claude was not found in PATH).")
            return 2
        result = run_command(
            [
                self.config.claude_bin,
                "-p",
                "/usage",
                "--output-format",
                "json",
                "--tools",
                "",
                "--no-session-persistence",
            ],
            check=False,
        )
        if result.returncode != 0:
            log("Claude quota unavailable: Claude Code's /usage command failed. Run 'claude auth login' if this persists.")
            return 2
        try:
            usage = str(json.loads(result.stdout).get("result") or "")
        except json.JSONDecodeError:
            usage = ""
        session = re.search(r"^Current session:\s*([0-9.]+)% used", usage, re.MULTILINE)
        week = re.search(r"^Current week(?: \([^)]*\))?:\s*([0-9.]+)% used", usage, re.MULTILINE)
        if not session or not week:
            log("Claude quota unavailable: Claude Code returned an unrecognized /usage format.")
            return 2
        session_remaining = 100 - float(session.group(1))
        week_remaining = 100 - float(week.group(1))
        log(f"Claude remaining quota — session: {session_remaining:g}%; week: {week_remaining:g}%.")
        return int(
            not (
                session_remaining >= self.config.minimum_remaining_percent
                and week_remaining >= self.config.minimum_remaining_percent
            )
        )

    def codex_capacity(self) -> int:
        if not command_available(self.config.codex_bin):
            log("Codex quota unavailable: codex was not found in PATH.")
            return 2
        result = run_command(
            [
                self.config.python_bin,
                self.config.script_dir / "codex_rate_limits.py",
                "--codex-bin",
                self.config.codex_bin,
            ],
            check=False,
        )
        if result.returncode != 0:
            log("Codex quota unavailable: the local rate-limit request failed.")
            return 2
        try:
            limits = json.loads(result.stdout)
            windows = [limits.get(key) for key in ("primary", "secondary") if limits.get(key) is not None]
            used = [float(window["usedPercent"]) for window in windows]
        except (json.JSONDecodeError, KeyError, TypeError, ValueError):
            log("Codex quota unavailable: the local rate-limit response was invalid.")
            return 2
        if not used:
            log("Codex quota unavailable: the local rate-limit response had no active windows.")
            return 2
        summary = "; ".join(
            f"{key}: {100 - float(limits[key]['usedPercent']):g}%"
            for key in ("primary", "secondary")
            if limits.get(key) is not None
        )
        log(f"Codex remaining quota — {summary}.")
        available = (
            limits.get("rateLimitReachedType") is None
            and not bool(limits.get("spendControlReached", False))
            and all(100 - amount >= self.config.minimum_remaining_percent for amount in used)
        )
        return 0 if available else 1

    def provider_capacity(self, provider: str) -> int:
        if provider.lower() == "claude":
            return self.claude_capacity()
        if provider.lower() == "codex":
            return self.codex_capacity()
        raise WorkerError(f"Invalid AI provider in saved state: {provider}")

    def choose_provider(self, previous_ai: str, available: dict[str, bool]) -> ProviderChoice | None:
        preferred = self.config.preferred_provider.capitalize()
        if previous_ai == "Claude":
            preferred = "Codex"
        elif previous_ai == "Codex":
            preferred = "Claude"
        order = [preferred, "Claude" if preferred == "Codex" else "Codex"]
        for provider in order:
            if available.get(provider, False):
                if previous_ai and provider != previous_ai:
                    log(f"Follow-up review prefers {provider} because {previous_ai} completed the previous pass.")
                elif previous_ai:
                    log(f"The alternate provider lacks capacity; falling back to {provider} for this follow-up.")
                return ProviderChoice(
                    name=provider,
                    model=self.config.claude_model if provider == "Claude" else self.config.codex_model,
                    effort=self.config.claude_effort if provider == "Claude" else self.config.codex_effort,
                    session_id=str(uuid.uuid4()) if provider == "Claude" else "",
                )
        return None

    def validate_paused_state(self, state: dict[str, Any]) -> None:
        required_strings = ("issue_title", "issue_url", "base_sha", "ai_tool", "model", "effort", "session_id")
        if not isinstance(state.get("issue_number"), int):
            raise WorkerError("Paused state has no numeric issue_number")
        for key in required_strings:
            if not isinstance(state.get(key), str) or not state[key]:
                raise WorkerError(f"Paused state has no valid {key}")
        if not SHA_RE.fullmatch(state["base_sha"]):
            raise WorkerError("Paused state has an invalid base_sha")
        for key in ("candidate_sha", "attempt_start_sha"):
            value = state.get(key, "")
            if value and (not isinstance(value, str) or not SHA_RE.fullmatch(value)):
                raise WorkerError(f"Paused state has an invalid {key}")
        if state["ai_tool"] not in {"Claude", "Codex"} or state.get("status") != "quota_paused":
            raise WorkerError("Paused state has an invalid provider or status")

    def resolve_recovery_candidate(self, base_sha: str, saved_sha: str, tip_sha: str) -> str:
        canonical = self.git("rev-parse", "--verify", f"{saved_sha}^{{commit}}", check=False)
        if (
            canonical
            and self.git_ok("merge-base", "--is-ancestor", base_sha, canonical)
            and self.git_ok("merge-base", "--is-ancestor", canonical, tip_sha)
        ):
            return canonical
        if not SHA_RE.fullmatch(saved_sha):
            raise WorkerError(f"Invalid recovery commit: {saved_sha}")
        prefix = saved_sha[:8]
        matches = [
            sha for sha in self.git("rev-list", tip_sha, f"^{base_sha}").splitlines() if sha.startswith(prefix)
        ]
        if len(matches) != 1:
            raise WorkerError(f"Cannot safely repair recovery commit: {saved_sha}")
        return matches[0]

    def normalize_recovery_commits(self, path: Path, tip_sha: str) -> dict[str, Any]:
        state = read_json(path)
        base = self.git("rev-parse", "--verify", f"{state.get('base_sha', '')}^{{commit}}", check=False)
        if not base or not self.git_ok("merge-base", "--is-ancestor", base, tip_sha):
            raise WorkerError(f"Saved base is invalid or not an ancestor of {tip_sha}")
        candidate = str(state.get("candidate_sha") or "")
        canonical_candidate = self.resolve_recovery_candidate(base, candidate, tip_sha) if candidate else ""
        attempt = str(state.get("attempt_start_sha") or "")
        canonical_attempt = self.git("rev-parse", "--verify", f"{attempt}^{{commit}}", check=False) if attempt else ""
        if attempt and (
            not canonical_attempt
            or not self.git_ok("merge-base", "--is-ancestor", base, canonical_attempt)
            or not self.git_ok("merge-base", "--is-ancestor", canonical_attempt, tip_sha)
        ):
            raise WorkerError("Saved attempt_start_sha is invalid or outside the recovery history")
        state["base_sha"] = base
        if canonical_candidate:
            state["candidate_sha"] = canonical_candidate
        else:
            state.pop("candidate_sha", None)
        if canonical_attempt:
            state["attempt_start_sha"] = canonical_attempt
        else:
            state.pop("attempt_start_sha", None)
        atomic_write_json(path, state)
        if candidate and candidate != canonical_candidate:
            log(f"Repaired saved recovery commit {candidate} as {canonical_candidate}.")
        return state

    def suspend_paused(self) -> None:
        state = self.read_state()
        self.validate_paused_state(state)
        current = self.git("rev-parse", "HEAD")
        state = self.normalize_recovery_commits(self.in_progress_file, current)
        issue_number = int(state["issue_number"])
        base = str(state["base_sha"])
        if not self.git_ok("merge-base", "--is-ancestor", base, current):
            raise WorkerError(f"Paused issue #{issue_number}'s branch no longer descends from its saved base commit")
        self.paused_dir.mkdir(parents=True, exist_ok=True)
        paused_file = self.paused_dir / f"{issue_number}.json"
        if paused_file.exists():
            raise WorkerError(f"A quota-paused state already exists for issue #{issue_number}: {paused_file}")
        stash_oid = ""
        if self.git("status", "--porcelain"):
            self.git("stash", "push", "--include-untracked", "--message", f"swarm issue worker paused #{issue_number}")
            stash_oid = self.git("rev-parse", "refs/stash")
            if self.git("status", "--porcelain"):
                raise WorkerError(f"Could not shelve all work for quota-paused issue #{issue_number}")
        attempt = str(state.get("attempt_start_sha") or "")
        candidate = str(state.get("candidate_sha") or "")
        if attempt and current != attempt:
            candidate = current
        elif not attempt and not candidate and state.get("quota_paused_at"):
            candidate = self.git(
                "rev-list", "-1", f"--before={state['quota_paused_at']}", current, check=False
            )
            if candidate == base:
                candidate = ""
        if candidate and not self.git_ok("merge-base", "--is-ancestor", base, candidate):
            raise WorkerError(f"Candidate commit for paused issue #{issue_number} is not after its base")
        if candidate:
            state["candidate_sha"] = candidate
        if stash_oid:
            state["worktree_stash_oid"] = stash_oid
        atomic_write_json(paused_file, state)
        self.in_progress_file.unlink()
        if self.config.delivery_mode == "pull-request":
            current_branch = self.git("branch", "--show-current")
            if current_branch != self.config.base_branch:
                self.git("switch", self.config.base_branch)
        log(f"Shelved quota-paused issue #{issue_number}; other ready issues may now run.")

    def restore_paused(self, paused_file: Path) -> None:
        if self.in_progress_file.exists():
            raise WorkerError("Cannot restore a quota-paused issue while another issue is active")
        state = read_json(paused_file)
        self.validate_paused_state(state)
        if self.git("status", "--porcelain"):
            raise WorkerError("Repository must be clean before restoring a quota-paused issue")
        if self.config.delivery_mode == "pull-request":
            branch = str(
                state.get("branch_name")
                or f"{self.config.branch_prefix}/{str(state['ai_tool']).lower()}/issue-{state['issue_number']}"
            )
            if self.git("branch", "--show-current") != branch:
                if self.git_ok("show-ref", "--verify", f"refs/heads/{branch}"):
                    self.git("switch", branch)
                else:
                    self.git("switch", "-c", branch, str(state["base_sha"]))
            state["branch_name"] = branch
            atomic_write_json(paused_file, state)
        state = self.normalize_recovery_commits(paused_file, self.git("rev-parse", "HEAD"))
        stash_oid = str(state.get("worktree_stash_oid") or "")
        issue_number = int(state["issue_number"])
        if stash_oid:
            if not self.git_ok("cat-file", "-e", f"{stash_oid}^{{commit}}"):
                raise WorkerError(f"Shelved work for issue #{issue_number} is missing: {stash_oid}")
            if not self.git_ok("stash", "apply", "--index", stash_oid):
                raise WorkerError(
                    f"Shelved work for issue #{issue_number} conflicts with newer commits; resolve manually"
                )
        state["status"] = "active"
        state["quota_resumed_at"] = iso_timestamp()
        state.pop("worktree_stash_oid", None)
        self.write_state(state)
        paused_file.unlink()
        if stash_oid:
            stash_list = self.git("stash", "list", "--format=%H %gd")
            reference = next((line.split()[1] for line in stash_list.splitlines() if line.split()[0] == stash_oid), "")
            if reference:
                self.git("stash", "drop", reference, check=False)

    def paused_files(self) -> list[Path]:
        files = sorted(self.paused_dir.glob("*.json")) if self.paused_dir.is_dir() else []
        for path in files:
            self.validate_paused_state(read_json(path))
        return files

    def issue_is_closed(self, issue_number: int) -> bool:
        issue = json.loads(
            self.github.gh(
                ["api", "--method", "GET", f"repos/{self.config.github_repository}/issues/{issue_number}"]
            )
        )
        return str(issue.get("state") or "").lower() == "closed"

    def archive_closed_paused(self, paused_file: Path) -> Path:
        state = read_json(paused_file)
        self.validate_paused_state(state)
        issue_number = int(state["issue_number"])
        self.closed_paused_dir.mkdir(parents=True, exist_ok=True)
        archive_file = self.closed_paused_dir / f"{issue_number}.json"
        if archive_file.exists():
            suffix = dt.datetime.now().astimezone().strftime("%Y%m%dT%H%M%S%z")
            archive_file = self.closed_paused_dir / f"{issue_number}-{suffix}.json"
        state.update(
            {
                "status": "closed_while_paused",
                "closed_detected_at": iso_timestamp(),
                "archived_from": paused_file.name,
            }
        )
        atomic_write_json(archive_file, state)
        paused_file.unlink()
        log(
            f"Issue #{issue_number} was closed while quota-paused; archived its saved attempt "
            "without running AI and continuing to the next eligible issue."
        )
        return archive_file

    def skip_closed_in_progress_pause(self) -> None:
        state = self.read_state()
        issue_number = int(state["issue_number"])
        self.suspend_paused()
        self.archive_closed_paused(self.paused_dir / f"{issue_number}.json")

    def mark_quota_paused(self) -> None:
        assert self.choice and self.issue
        state = self.read_state()
        if state.get("status") == "quota_paused":
            return
        state.update(
            {
                "status": "quota_paused",
                "quota_paused_at": iso_timestamp(),
                "quota_pause_count": int(state.get("quota_pause_count", 0)) + 1,
                "quota_comment_posted": False,
                "quota_email_sent": False,
            }
        )
        self.write_state(state)
        log(
            f"Paused issue #{self.issue.number} because {self.choice.name} usage is unavailable; "
            f"session {self.choice.session_id} was preserved."
        )

    def post_quota_comment(self) -> None:
        assert self.choice and self.issue
        state = self.read_state()
        if state.get("quota_comment_posted"):
            return
        pause_count = int(state.get("quota_pause_count", 1))
        marker = (
            f"<!-- swarm-issue-worker:quota-paused:issue:{self.issue.number};"
            f"pause:{pause_count};session:{self.choice.session_id} -->"
        )
        existing = any(marker in str(comment.get("body") or "") for comment in self.comments(self.issue.number))
        if not existing:
            body = (
                f"{marker}\nWork paused because **{self.choice.name}** no longer has sufficient usage available.\n\n"
                f"- Model: `{self.choice.model}`\n- Session: `{self.choice.session_id}`\n"
                "- The current work and AI session were saved.\n"
                f"- The worker will wait for {self.choice.name} specifically, include new trusted comments, "
                "and resume this same session automatically.\n"
            )
            log(f"Posting the one-time quota pause notice to GitHub issue #{self.issue.number}.")
            self.github.gh(
                ["issue", "comment", str(self.issue.number), "--repo", self.config.github_repository, "--body-file", "-"],
                self.choice.key,
                body,
            )
        self.update_state(quota_comment_posted=True)

    def send_notification(self, state: dict[str, Any], notification_type: str = "completed") -> None:
        if self.config.no_email:
            log(f"Email notification disabled for issue #{state['issue_number']}.")
            return
        if not self.config.smtp_credentials_file:
            raise WorkerError("Set --smtp-credentials-file (or SWARM_SMTP_CREDENTIALS_FILE) to send notifications")
        if not self.config.smtp_credentials_file.is_file():
            raise WorkerError(f"SMTP settings file was not found: {self.config.smtp_credentials_file}")
        if not self.config.smtp_password:
            raise WorkerError("SMTP password must be supplied by the foreground runner")
        command: list[str | Path] = [
            self.config.python_bin,
            self.config.script_dir / "send_issue_notification.py",
            "--credentials",
            self.config.smtp_credentials_file,
            "--password-stdin",
            "--to",
            self.config.email_to,
            "--issue-number",
            str(state["issue_number"]),
            "--issue-title",
            str(state["issue_title"]),
            "--issue-url",
            str(state["issue_url"]),
            "--ai",
            str(state.get("ai_tool") or state.get("ai")),
        ]
        if notification_type == "quota-paused":
            command.extend(
                [
                    "--notification-type",
                    "quota-paused",
                    "--model",
                    str(state["model"]),
                    "--session-id",
                    str(state["session_id"]),
                ]
            )
        else:
            command.extend(
                [
                    "--commit-sha",
                    str(state["commit_sha"]),
                    "--commit-message",
                    str(state["commit_message"]),
                ]
            )
        run_command(command, input_text=f"{self.config.smtp_password}\n")

    def deliver_quota_notifications(self) -> None:
        self.post_quota_comment()
        state = self.read_state()
        if not state.get("quota_email_sent"):
            log(f"Sending the one-time quota pause notification for issue #{state['issue_number']}.")
            self.send_notification(state, "quota-paused")
            self.update_state(quota_email_sent=True)

    def started_comment_marker(self) -> str:
        assert self.issue and self.choice
        return (
            f"<!-- swarm-issue-worker:started:issue:{self.issue.number};"
            f"provider:{self.choice.key};branch:{self.expected_branch()} -->"
        )

    def post_started_comment(self) -> None:
        assert self.issue and self.choice
        state = self.read_state()
        if state.get("started_comment_posted"):
            return
        marker = self.started_comment_marker()
        already_posted = any(
            marker in str(comment.get("body") or "")
            for comment in self.comments(self.issue.number)
        )
        if not already_posted:
            action = "started follow-up work on" if self.issue.work_type == "followup" else "started working on"
            body = (
                f"{marker}\n🤖 **{self.choice.name} Bot** {action} this issue.\n\n"
                f"- Model: `{self.choice.model}`\n"
                f"- Branch: `{self.expected_branch()}`\n"
            )
            self.github.gh(
                [
                    "issue",
                    "comment",
                    str(self.issue.number),
                    "--repo",
                    self.config.github_repository,
                    "--body-file",
                    "-",
                ],
                self.choice.key,
                body,
            )
            log(f"Posted {self.choice.name} Bot start notice to issue #{self.issue.number}.")
        else:
            log(f"Issue #{self.issue.number} already has this work-round start notice.")
        self.update_state(started_comment_posted=True)

    def ai_failure_is_quota(self) -> bool:
        combined = ""
        for path in (self.ai_output_file, self.ai_diagnostic_file):
            if path.exists():
                combined += path.read_text(encoding="utf-8", errors="replace")
        if QUOTA_RE.search(combined):
            return True
        assert self.choice
        return self.provider_capacity(self.choice.name) == 1

    def load_resume_comments(self, issue_number: int, after_id: int) -> list[dict[str, Any]]:
        return [
            {
                "id": int(comment["id"]),
                "author": str((comment.get("user") or {}).get("login") or "unknown"),
                "created_at": str(comment.get("created_at") or ""),
                "body": str(comment.get("body") or ""),
            }
            for comment in sorted(self.comments(issue_number), key=lambda item: int(item["id"]))
            if int(comment["id"]) > after_id
            and not is_worker_comment(comment)
            and str((comment.get("user") or {}).get("login") or "") in self.trusted_followup_authors
        ]

    def save_new_state(self, issue: IssueContext, choice: ProviderChoice, base_sha: str) -> None:
        self.write_state(
            {
                "issue_number": issue.number,
                "issue_title": issue.title,
                "issue_url": issue.url,
                "base_sha": base_sha,
                "branch_name": self.expected_branch(),
                "work_type": issue.work_type,
                "previous_commit_sha": issue.previous_commit_sha,
                "previous_ai": issue.previous_ai,
                "previous_completion_comment": issue.previous_completion_comment,
                "followup_comments": issue.followup_comments,
                "trigger_comment_id": issue.trigger_comment_id,
                "ai_tool": choice.name,
                "model": choice.model,
                "effort": choice.effort,
                "session_id": choice.session_id,
                "session_comment_id": issue.trigger_comment_id or 0,
                "status": "active",
                "quota_pause_count": 0,
                "started_at": iso_timestamp(),
            }
        )

    def issue_from_state(self, state: dict[str, Any], remote_issue: dict[str, Any]) -> IssueContext:
        return IssueContext(
            number=int(state["issue_number"]),
            title=str(remote_issue["title"]),
            body=str(remote_issue.get("body") or ""),
            labels=[str(label["name"]) for label in remote_issue.get("labels", [])],
            url=str(remote_issue["html_url"]),
            work_type=str(state.get("work_type") or "initial"),
            previous_commit_sha=str(state.get("previous_commit_sha") or ""),
            previous_ai=str(state.get("previous_ai") or ""),
            previous_completion_comment=state.get("previous_completion_comment"),
            followup_comments=list(state.get("followup_comments") or []),
            trigger_comment_id=state.get("trigger_comment_id"),
        )

    def assigned_issues(self) -> list[dict[str, Any]]:
        issues = self.github.api_list(
            f"repos/{self.config.github_repository}/issues",
            {
                "state": "open",
                "assignee": self.config.github_assignee,
                "sort": "created",
                "direction": "asc",
                "per_page": 100,
            },
        )
        return sorted(
            [
                issue
                for issue in issues
                if "pull_request" not in issue
                and any(
                    assignee.get("login") == self.config.github_assignee
                    for assignee in issue.get("assignees", [])
                )
            ],
            key=lambda issue: int(issue["number"]),
        )

    def record_completed_from_comments(self, issue_number: int, comments: list[dict[str, Any]]) -> bool:
        metadata = extract_completion_metadata(comments, self.completion_authors)
        if not metadata:
            return False
        commit_sha = str(metadata["commit_sha"])
        main_sha = self.git(
            "rev-parse", "--verify", f"refs/heads/{self.config.base_branch}", check=False
        )
        if (
            not main_sha
            or not self.git_ok("cat-file", "-e", f"{commit_sha}^{{commit}}")
            or not self.git_ok("merge-base", "--is-ancestor", commit_sha, main_sha)
        ):
            log(
                f"Issue #{issue_number} has a trusted completion marker for {commit_sha}, but that commit "
                f"is not on local {self.config.base_branch}; leaving it pending until synchronized."
            )
            return False
        self.record_completed(issue_number)
        self.clear_in_progress(issue_number)
        log(f"Recognized issue #{issue_number} as already completed by commit {commit_sha} on main; no AI run is needed.")
        return True

    def select_issue(self) -> IssueContext | None:
        issues = self.assigned_issues()
        completed = self.completed_numbers()
        paused = {int(path.stem) for path in self.paused_files()}
        in_progress_number: int | None = None
        saved_state: dict[str, Any] | None = None
        if self.in_progress_file.exists():
            saved_state = self.read_state()
            in_progress_number = int(saved_state["issue_number"])
            if str(saved_state.get("work_type") or "initial") == "initial" and in_progress_number in completed:
                self.clear_in_progress(in_progress_number)
                saved_state = None
                in_progress_number = None

        for candidate in issues:
            number = int(candidate["number"])
            if number in completed or number in paused:
                continue
            comments = self.comments(number)
            if self.record_completed_from_comments(number, comments):
                completed.add(number)
                if in_progress_number == number:
                    in_progress_number = None
                    saved_state = None

        if in_progress_number is not None:
            remote = next((item for item in issues if int(item["number"]) == in_progress_number), None)
            if remote is None:
                raise WorkerError(
                    f"Saved in-progress issue #{in_progress_number} is no longer open and assigned to "
                    f"{self.config.github_assignee}; review {self.in_progress_file}."
                )
            assert saved_state is not None
            return self.issue_from_state(saved_state, remote)

        ready: list[tuple[int, str, dict[str, Any], dict[str, Any] | None]] = []
        for candidate in issues:
            number = int(candidate["number"])
            if number in paused:
                continue
            if number not in completed:
                ready.append((number, "initial", candidate, None))
                continue
            metadata = extract_followup_metadata(
                self.comments(number), self.trusted_followup_authors, self.completion_authors
            )
            if not metadata:
                continue
            if not SHA_RE.fullmatch(str(metadata["previous_commit_sha"])):
                raise WorkerError(
                    f"Latest completion on issue #{number} has no valid completion commit"
                )
            ready.append((number, "followup", candidate, metadata))

        if ready:
            _, work_type, remote, metadata = min(ready, key=lambda item: item[0])
            if work_type == "initial":
                return IssueContext(
                    number=int(remote["number"]),
                    title=str(remote["title"]),
                    body=str(remote.get("body") or ""),
                    labels=[str(label["name"]) for label in remote.get("labels", [])],
                    url=str(remote["html_url"]),
                )
            assert metadata is not None
            return IssueContext(
                number=int(remote["number"]),
                title=str(remote["title"]),
                body=str(remote.get("body") or ""),
                labels=[str(label["name"]) for label in remote.get("labels", [])],
                url=str(remote["html_url"]),
                work_type="followup",
                previous_commit_sha=str(metadata["previous_commit_sha"]),
                previous_ai=str(metadata["previous_ai"]),
                previous_completion_comment=metadata["previous_completion_comment"],
                followup_comments=metadata["followup_comments"],
                trigger_comment_id=int(metadata["trigger_comment_id"]),
            )
        if paused:
            log("No other issue can be worked now; quota-paused issues remain safely shelved.")
        else:
            log(f"No new issue or follow-up comment assigned to {self.config.github_assignee} was found.")
        return None

    def choice_from_state(self, state: dict[str, Any]) -> ProviderChoice:
        # `session_id` for Claude is generated and persisted (in
        # save_new_state, via prepare_repository) before the `claude`
        # process is ever actually invoked in run_ai — so its mere presence
        # doesn't mean a resumable session exists yet. `session_started` is
        # only set (in run_ai) once that first invocation genuinely happens;
        # gating resume on both prevents a crash between those two points
        # (e.g. post_started_comment failing) from producing a retry that
        # tries to --resume a session ID that was never actually started.
        return ProviderChoice(
            name=str(state["ai_tool"]),
            model=str(state["model"]),
            effort=str(state["effort"]),
            session_id=str(state.get("session_id") or ""),
            resume=bool(state.get("session_id")) and bool(state.get("session_started")),
        )

    def prepare_paused_resume(self) -> bool:
        if self.in_progress_file.exists():
            state = self.read_state()
            if state.get("status") == "quota_paused":
                self.validate_paused_state(state)
                issue_number = int(state["issue_number"])
                if self.issue_is_closed(issue_number):
                    if self.config.dry_run:
                        log(
                            f"Dry run: issue #{issue_number} is closed; would archive its quota-paused "
                            "attempt without running AI."
                        )
                        return True
                    self.skip_closed_in_progress_pause()
                else:
                    self.issue = IssueContext(
                        int(state["issue_number"]), str(state["issue_title"]), "", [], str(state["issue_url"])
                    )
                    self.choice = self.choice_from_state(state)
                    if not self.config.dry_run:
                        self.deliver_quota_notifications()
                    capacity = self.provider_capacity(self.choice.name)
                    if capacity != 0:
                        if self.config.dry_run:
                            log(f"Dry run: issue #{self.issue.number} remains quota-paused on {self.choice.name}.")
                            return True
                        self.suspend_paused()
                        raise SystemExit(QUOTA_PAUSED_EXIT_CODE)
                    if self.config.dry_run:
                        log(
                            f"Dry run: would resume {self.choice.name} session {self.choice.session_id} "
                            f"for issue #{self.issue.number}."
                        )
                        return True
                    self.update_state(status="active", quota_resumed_at=iso_timestamp())
                    self.choice.resume = True
                    self.quota_resume_ready = True
                    log(
                        f"{self.choice.name} usage is available again; preparing to resume session "
                        f"{self.choice.session_id} for issue #{self.issue.number}."
                    )
                    return False

        if not self.in_progress_file.exists():
            for paused_file in self.paused_files():
                state = read_json(paused_file)
                issue_number = int(state["issue_number"])
                if self.issue_is_closed(issue_number):
                    if self.config.dry_run:
                        log(
                            f"Dry run: issue #{issue_number} is closed; would archive its quota-paused "
                            "attempt without running AI."
                        )
                        continue
                    self.archive_closed_paused(paused_file)
                    continue
                provider = str(state["ai_tool"])
                if self.provider_capacity(provider) != 0:
                    continue
                if self.config.dry_run:
                    log(
                        f"Dry run: would restore quota-paused issue #{state['issue_number']} "
                        f"with its pinned {provider} session."
                    )
                    return True
                self.restore_paused(paused_file)
                restored = self.read_state()
                self.choice = self.choice_from_state(restored)
                self.choice.resume = True
                self.quota_resume_ready = True
                log(
                    f"{provider} usage is available again; restored session {self.choice.session_id} "
                    f"for issue #{restored['issue_number']}."
                )
                break
        return False

    def render_pending_comment(self, pending: dict[str, Any]) -> str:
        commit_sha = str(pending["commit_sha"])
        marker = f"<!-- swarm-issue-worker:commit:{commit_sha}"
        trigger = pending.get("trigger_comment_id")
        if isinstance(trigger, int) and trigger > 0:
            marker += f";through-comment:{trigger}"
        marker += " -->"
        verb = "Reworked" if pending.get("work_type") == "followup" else "Completed"
        return (
            f"{marker}\n{verb} by **{pending.get('ai_tool') or pending.get('ai')}**.\n\n"
            f"- Model: `{pending.get('model', 'unknown')}`\n"
            f"- Effort: `{pending.get('effort', 'unknown')}`\n"
            f"- Commit: `{commit_sha}` — {pending['commit_message']}\n\n"
            "<details><summary>AI completion summary</summary>\n\n"
            f"{pending.get('ai_output') or '(No captured AI output was available.)'}\n"
            "</details>\n"
        )

    def post_pending_comment(self, pending: dict[str, Any]) -> dict[str, Any]:
        if pending.get("github_comment_posted"):
            return pending
        issue_number = int(pending["issue_number"])
        body = self.render_pending_comment(pending)
        marker = body.splitlines()[0]
        already_posted = any(marker in str(comment.get("body") or "") for comment in self.comments(issue_number))
        provider = str(pending.get("ai_tool") or pending.get("ai") or "").lower()
        if not already_posted:
            log(f"Posting the AI completion summary to GitHub issue #{issue_number}.")
            self.github.gh(
                ["issue", "comment", str(issue_number), "--repo", self.config.github_repository, "--body-file", "-"],
                provider,
                body,
            )
        else:
            log(f"GitHub issue #{issue_number} already has the completion response for {pending['commit_sha']}.")
        pending["github_comment_posted"] = True
        atomic_write_json(self.pending_file, pending)
        return pending

    def add_pending_label(self, pending: dict[str, Any]) -> dict[str, Any]:
        if pending.get("ready_for_testing_label_added"):
            return pending
        provider = str(pending.get("ai_tool") or pending.get("ai") or "").lower()
        issue_number = int(pending["issue_number"])
        log(f"Adding the '{self.config.ready_label}' label to GitHub issue #{issue_number}.")
        self.github.gh(
            [
                "issue",
                "edit",
                str(issue_number),
                "--repo",
                self.config.github_repository,
                "--add-label",
                self.config.ready_label,
            ],
            provider,
        )
        pending["ready_for_testing_label_added"] = True
        atomic_write_json(self.pending_file, pending)
        return pending

    def deliver_pending(self) -> None:
        if not self.pending_file.exists():
            return
        if self.config.dry_run:
            log("Dry run: a pending notification exists; no email or AI work was performed.")
            raise SystemExit(0)
        pending = read_json(self.pending_file)
        pending = self.post_pending_comment(pending)
        pending = self.add_pending_label(pending)
        issue_number = int(pending["issue_number"])
        log(f"Sending notification for issue #{issue_number}.")
        self.send_notification(pending)
        self.record_completed(issue_number)
        self.pending_file.unlink()
        self.clear_in_progress(issue_number)
        log(f"Notification sent; issue #{issue_number} is marked completed locally.")

    def build_prompt(
        self,
        recovery_mode: bool,
        recovery_candidate: str,
        recovery_dirty: bool,
    ) -> str:
        assert self.issue and self.choice
        issue = self.issue
        state = self.read_state()
        lines: list[str] = []
        if self.choice.resume:
            comments = self.load_resume_comments(issue.number, int(state.get("session_comment_id", 0)))
            lines.extend(
                [
                    f"Continue the existing unattended session for GitHub issue #{issue.number} "
                    f"({issue.title}) from exactly where the previous turn stopped when usage became unavailable.",
                    "Inspect and preserve work already present in the repository, finish the implementation "
                    f"and foreground verification on {self.expected_branch()}. Do not switch branches, push, or "
                    "ask for interactive input. The worker will commit any completed changes you leave uncommitted.",
                ]
            )
            if comments:
                lines.append("\nTrusted GitHub comments added since this session last received issue context:")
                for comment in comments:
                    lines.append(
                        f"Comment #{comment['id']} by @{comment['author']} at {comment['created_at']}:\n"
                        f"{comment['body']}\n"
                    )
                lines.append("Treat these comments as additional requirements or corrections for the work you are continuing.")
                log(f"Adding trusted issue comments through comment {comments[-1]['id']} to the resumed session.")
                self.update_state(session_comment_id=int(comments[-1]["id"]))
        else:
            lines.extend(
                [
                    "Issue title:",
                    issue.title,
                    "Issue number:",
                    f"#{issue.number}",
                    "",
                    "Issue description:",
                    issue.body,
                    "",
                    "Issue tags:",
                    ", ".join(issue.labels) or "none",
                    "",
                    AUTOPILOT_INSTRUCTION,
                    f"Implement this issue in {self.config.repo_dir}. Follow repository instructions, run relevant "
                    f"tests, and remain on {self.expected_branch()}. You may commit, but do not push; the worker "
                    f"will commit any completed changes you leave uncommitted and ensure #{issue.number} is in "
                    "the commit message. Run verification commands in the "
                    "foreground; do not return while tests or builds are still running.",
                ]
            )
            if issue.work_type == "followup":
                lines.extend(
                    [
                        "\nFollow-up rework context:",
                        "This issue was previously worked, but new GitHub comments indicate that it needs another "
                        "pass. Treat them as refinement or defect feedback. Reinspect the implementation, make the "
                        f"additional fix, and verify it. The worker will create a commit if needed.",
                    ]
                )
                if issue.previous_ai and self.choice.name != issue.previous_ai:
                    lines.append(
                        f"The previous pass was completed by {issue.previous_ai}. You are intentionally providing an "
                        "independent second-provider review; challenge prior assumptions and use issue comments and "
                        "repository evidence as the source of truth."
                    )
                previous_show = self.git(
                    "show", "--no-ext-diff", "--format=fuller", "--stat", "--summary", issue.previous_commit_sha
                )
                lines.extend(
                    [
                        "\nPrevious completion commit and change summary:",
                        previous_show,
                        f"\nInspect the complete previous patch with: git show --no-ext-diff {issue.previous_commit_sha}",
                        "\nPrevious worker completion comment:",
                        self.format_comment(issue.previous_completion_comment or {}),
                        "\nNew GitHub follow-up comments to address, in order:",
                    ]
                )
                lines.extend(self.format_comment(comment) for comment in issue.followup_comments)
        lines.append(SUMMARY_INSTRUCTION)
        if recovery_dirty:
            lines.append(
                "The worktree also contains uncommitted changes. Inspect them, but do not assume they belong to "
                "this issue: preserve unrelated changes exactly. If any are unfinished work for this issue, finish "
                "and commit only that issue work."
            )
        if recovery_mode and recovery_candidate:
            lines.append(
                f"This is a recovery verification run. Commit {recovery_candidate} was created after the original "
                "attempt began and may already implement this issue. Verify implementation and tests. If complete, "
                "do not duplicate code or rewrite history; put SWARM_RECOVERY_COMPLETE on its own final line after "
                "your summary. If incomplete, finish it; the worker will commit any remaining completed changes."
            )
        return "\n".join(lines) + "\n"

    @staticmethod
    def format_comment(comment: dict[str, Any]) -> str:
        return (
            f"Comment #{comment.get('id')} by @{comment.get('author', 'unknown')} at "
            f"{comment.get('created_at', '')}:\n{comment.get('body', '')}\n"
        )

    def expected_branch(self) -> str:
        if self.config.delivery_mode == "pull-request":
            assert self.issue and self.choice
            suffix = ""
            if self.issue.work_type == "followup" and self.issue.trigger_comment_id:
                suffix = f"-followup-{self.issue.trigger_comment_id}"
            return f"{self.config.branch_prefix}/{self.choice.key}/issue-{self.issue.number}{suffix}"
        return self.config.base_branch

    def review_provider(self) -> str:
        assert self.choice
        return "codex" if self.choice.key == "claude" else "claude"

    def provider_environment(self) -> dict[str, str]:
        assert self.choice
        environment = self.github.environment(self.choice.key)
        # SMTP secrets must never be inherited by Claude or Codex.
        environment["SWARM_SMTP_PASSWORD"] = ""
        return environment

    def run_ai(self, prompt: str) -> int:
        assert self.choice
        self.ai_output_file.write_text("", encoding="utf-8")
        self.ai_diagnostic_file.write_text("", encoding="utf-8")
        env = os.environ.copy()
        env.update(self.provider_environment())
        env.pop("SWARM_SMTP_PASSWORD", None)
        if self.choice.name == "Claude":
            if not self.config.claude_bin:
                raise WorkerError("Claude executable is unavailable")
            command = [
                self.config.claude_bin,
                "--model",
                self.choice.model,
                "--effort",
                self.choice.effort,
                "--permission-mode",
                "bypassPermissions",
            ]
            command.extend(
                ["--resume", self.choice.session_id]
                if self.choice.resume
                else ["--session-id", self.choice.session_id]
            )
            command.extend(["-p", "-"])
            process = subprocess.Popen(
                command,
                cwd=self.config.repo_dir,
                env=env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
            # The process now genuinely owns self.choice.session_id (created
            # via --session-id, or attached via --resume) — from here on a
            # retry may legitimately --resume it. See choice_from_state.
            self.update_state(session_started=True)
            assert process.stdin and process.stdout
            process.stdin.write(prompt)
            process.stdin.close()
            with self.ai_output_file.open("w", encoding="utf-8") as output:
                for line in process.stdout:
                    print(line, end="", flush=True)
                    output.write(line)
            return process.wait()

        if not self.config.codex_bin:
            raise WorkerError("Codex executable is unavailable")
        log("Codex is working. Detailed implementation output is hidden; its final summary will appear when finished.")
        command = [self.config.codex_bin, "exec"]
        if self.choice.resume:
            command.append("resume")
        command.extend(
            [
                "-m",
                self.choice.model,
                "--dangerously-bypass-approvals-and-sandbox",
                "--dangerously-bypass-hook-trust",
                "-c",
                f'model_reasoning_effort="{self.choice.effort}"',
            ]
        )
        if not self.choice.resume:
            command.extend(["-C", str(self.config.repo_dir)])
        command.extend(["--json", "--output-last-message", str(self.ai_output_file)])
        if self.choice.resume:
            command.append(self.choice.session_id)
        command.append("-")
        with self.ai_diagnostic_file.open("w", encoding="utf-8") as diagnostic:
            result = subprocess.run(
                command,
                cwd=self.config.repo_dir,
                env=env,
                input=prompt,
                text=True,
                stdout=diagnostic,
                stderr=subprocess.STDOUT,
                check=False,
            )
        for line in self.ai_diagnostic_file.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get("type") == "thread.started" and event.get("thread_id"):
                self.choice.session_id = str(event["thread_id"])
                self.update_state(session_id=self.choice.session_id, session_started=True)
                break
        return result.returncode

    def synchronize_base_branch(self) -> str:
        current = self.git("branch", "--show-current")
        if current != self.config.base_branch:
            raise WorkerError(
                f"Cannot synchronize {self.config.base_branch} while checked out on {current or 'detached HEAD'}"
            )
        if self.git("status", "--porcelain"):
            raise WorkerError(f"Cannot synchronize dirty {self.config.base_branch}")
        self.git("pull", "--ff-only", self.config.remote_name, self.config.base_branch)
        synchronized = self.git("rev-parse", "HEAD")
        log(
            f"Synchronized local {self.config.base_branch} with "
            f"{self.config.remote_name}/{self.config.base_branch} at {synchronized}."
        )
        return synchronized

    def prune_merged_worker_branches(self) -> None:
        pattern = re.compile(
            rf"^{re.escape(self.config.branch_prefix)}/(?:claude|codex)/"
            r"issue-[0-9]+(?:-followup-[0-9]+)?$"
        )
        branches = self.git("for-each-ref", "--format=%(refname:short)", "refs/heads").splitlines()
        for branch in branches:
            if not pattern.fullmatch(branch):
                continue
            if self.git_ok("merge-base", "--is-ancestor", branch, self.config.base_branch):
                self.git("branch", "-D", branch, check=False)
                log(f"Removed merged local worker branch {branch}.")

    def prepare_repository(self) -> tuple[str, bool, str, bool]:
        assert self.issue and self.choice
        if not self.git_ok("rev-parse", "--is-inside-work-tree"):
            raise WorkerError(f"--repo-dir is not a Git repository: {self.config.repo_dir}")
        current_branch = self.git("branch", "--show-current")
        expected = self.expected_branch()
        state_exists = self.in_progress_file.exists()
        if self.config.delivery_mode == "local-main":
            if current_branch != self.config.base_branch:
                if self.git("status", "--porcelain"):
                    raise WorkerError("Repository has changes on a non-main branch; refusing unattended recovery")
                self.git("switch", self.config.base_branch)
            if not state_exists:
                if self.git("status", "--porcelain"):
                    log("Repository has uncommitted changes unrelated to a saved attempt; deferring new issue work.")
                    raise SystemExit(0)
                self.synchronize_base_branch()
        elif not state_exists:
            if current_branch != self.config.base_branch:
                if self.git("status", "--porcelain"):
                    raise WorkerError("Repository has changes on a non-main branch with no recovery state")
                self.git("switch", self.config.base_branch)
            if self.git("status", "--porcelain"):
                log("Repository has uncommitted changes unrelated to a saved attempt; deferring new issue work.")
                raise SystemExit(0)
            base = self.synchronize_base_branch()
            self.prune_merged_worker_branches()
            # Persist ownership before creating the branch. If interrupted
            # between these operations, the next run recreates the branch from
            # this exact base instead of treating it as orphaned work.
            self.save_new_state(self.issue, self.choice, base)
            if self.git_ok("show-ref", "--verify", f"refs/heads/{expected}"):
                if self.git_ok("merge-base", "--is-ancestor", expected, self.config.base_branch):
                    self.git("branch", "-D", expected)
                else:
                    raise WorkerError(
                        f"Existing branch {expected} contains unmerged work; recovery state was preserved"
                    )
            self.git("switch", "-c", expected, base)
            log(f"Created issue branch {expected} from {self.config.base_branch} at {base}.")
        else:
            state = self.read_state()
            expected = str(state.get("branch_name") or expected)
            if current_branch != expected:
                if self.git("status", "--porcelain"):
                    raise WorkerError(f"Repository must be on saved issue branch {expected} before recovery")
                if self.git_ok("show-ref", "--verify", f"refs/heads/{expected}"):
                    self.git("switch", expected)
                else:
                    self.git("switch", "-c", expected, str(state["base_sha"]))
                    log(f"Recreated interrupted issue branch {expected} from its saved base.")

        run_start = self.git("rev-parse", "HEAD")
        base = run_start
        recovery_mode = False
        candidate = ""
        recovery_dirty = False
        if self.issue.work_type == "followup":
            if not self.git_ok("cat-file", "-e", f"{self.issue.previous_commit_sha}^{{commit}}"):
                raise WorkerError(
                    f"Previous completion commit is unavailable locally: {self.issue.previous_commit_sha}"
                )
            if not self.git_ok("merge-base", "--is-ancestor", self.issue.previous_commit_sha, run_start):
                raise WorkerError(
                    f"Current branch does not contain previous issue completion {self.issue.previous_commit_sha}"
                )
        if state_exists:
            state = self.normalize_recovery_commits(self.in_progress_file, run_start)
            if int(state["issue_number"]) != self.issue.number:
                raise WorkerError(
                    f"Issue #{state['issue_number']} is already in progress; refusing issue #{self.issue.number}"
                )
            base = str(state["base_sha"])
            candidate = str(state.get("candidate_sha") or "")
            recovery_mode = True
            if run_start != base:
                if not candidate:
                    candidate = run_start
                    self.update_state(candidate_sha=candidate)
                log(f"Verifying commit {candidate} as recovered implementation for issue #{self.issue.number}.")
            if self.git("status", "--porcelain"):
                recovery_dirty = True
                log(f"Preserving uncommitted work while recovering issue #{self.issue.number}.")
            elif not candidate:
                log(f"Retrying issue #{self.issue.number} from its original clean base commit.")
        else:
            if self.git("status", "--porcelain"):
                log("Repository has uncommitted changes unrelated to a saved attempt; deferring new issue work.")
                raise SystemExit(0)
            if not self.in_progress_file.exists():
                self.save_new_state(self.issue, self.choice, base)
        self.update_state(attempt_start_sha=run_start, branch_name=expected)
        return run_start, recovery_mode, candidate, recovery_dirty

    def commit_completed_work(self, run_start: str) -> str:
        assert self.issue and self.choice
        if not self.git("status", "--porcelain"):
            return self.git("rev-parse", "HEAD")
        self.git("add", "--all")
        if self.git_ok("diff", "--cached", "--quiet"):
            raise WorkerError(
                f"Issue #{self.issue.number} left worktree changes that Git could not stage"
            )
        current = self.git("rev-parse", "HEAD")
        if current == run_start:
            title = re.sub(r"\s+", " ", self.issue.title).strip()
            message = f"{title} (#{self.issue.number})"
        else:
            message = f"Commit remaining completed work (#{self.issue.number})"
        run_command(
            [
                self.config.git_bin,
                "-C",
                self.config.repo_dir,
                "commit",
                "--no-verify",
                "-m",
                message,
            ],
            env=self.provider_environment(),
        )
        committed = self.git("rev-parse", "HEAD")
        if self.git("status", "--porcelain"):
            raise WorkerError(
                f"Issue #{self.issue.number} still has uncommitted changes after worker commit"
            )
        log(f"Committed completed issue #{self.issue.number} work as {committed}.")
        return committed

    def ensure_issue_reference(self, commit_sha: str, recovered: bool) -> str:
        assert self.issue and self.choice
        body = self.git("log", "-1", "--format=%B", commit_sha)
        if re.search(rf"(^|[^0-9])#{self.issue.number}([^0-9]|$)", body):
            return commit_sha
        if recovered:
            log(
                f"Recovered commit {commit_sha} is established and does not mention #{self.issue.number}; "
                "leaving history unchanged."
            )
            return commit_sha
        lines = body.splitlines()
        lines[0] = f"{lines[0]} (#{self.issue.number})"
        environment = self.provider_environment()
        run_command(
            [self.config.git_bin, "-C", self.config.repo_dir, "commit", "--amend", "--no-verify", "-F", "-"],
            env=environment,
            input_text="\n".join(lines) + "\n",
        )
        amended = self.git("rev-parse", "HEAD")
        log(f"Added issue #{self.issue.number} to the commit message.")
        return amended

    def approve_pull_request(self, pr_url: str) -> str:
        reviewer = self.review_provider()
        self.github.gh(
            [
                "pr",
                "review",
                pr_url,
                "--repo",
                self.config.github_repository,
                "--approve",
                "--body",
                "Automated approval after the implementing provider completed verification.",
            ],
            reviewer,
        )
        log(f"{reviewer.capitalize()} Bot approved {pr_url}.")
        return reviewer

    def return_to_synchronized_main(self, branch: str) -> str:
        if self.git("status", "--porcelain"):
            raise WorkerError("Cannot finish PR delivery while the issue branch is dirty")
        if self.git("branch", "--show-current") != self.config.base_branch:
            self.git("switch", self.config.base_branch)
        synchronized = self.synchronize_base_branch()
        if branch and branch != self.config.base_branch:
            self.git("branch", "-D", branch, check=False)
        if self.git("branch", "--show-current") != self.config.base_branch:
            raise WorkerError(f"PR delivery did not return the checkout to {self.config.base_branch}")
        if self.git("status", "--porcelain"):
            raise WorkerError(f"PR delivery left {self.config.base_branch} dirty")
        log(f"Returned the clean local checkout to {self.config.base_branch} at {synchronized}.")
        return synchronized

    def deliver_pull_request(self, commit_sha: str) -> tuple[str, str, str]:
        assert self.issue and self.choice
        branch = self.expected_branch()
        environment = self.provider_environment()
        existing_text = self.github.gh(
            [
                "pr",
                "list",
                "--repo",
                self.config.github_repository,
                "--head",
                branch,
                "--state",
                "all",
                "--limit",
                "1",
                "--json",
                "url,state,mergeCommit",
            ],
            self.choice.key,
        )
        existing = json.loads(existing_text)
        if existing and existing[0].get("state") == "MERGED":
            pr_url = str(existing[0]["url"])
            delivered_sha = str((existing[0].get("mergeCommit") or {}).get("oid") or "")
            if not SHA_RE.fullmatch(delivered_sha):
                raise WorkerError(f"Merged PR did not report a valid merge commit: {pr_url}")
            self.return_to_synchronized_main(branch)
            log(f"Recovered already-merged pull request {pr_url} for issue #{self.issue.number}.")
            return pr_url, branch, delivered_sha

        # GIT_ASKPASS keeps the installation token out of command-line
        # arguments, Git configuration, and persistent remote URLs.
        with tempfile.TemporaryDirectory(prefix="swarm-git-askpass.") as temporary:
            askpass = Path(temporary) / "askpass.sh"
            askpass.write_text(
                "#!/bin/sh\n"
                "case \"$1\" in\n"
                "  *Username*) printf '%s\\n' x-access-token ;;\n"
                "  *) printf '%s\\n' \"$SWARM_GITHUB_APP_PUSH_TOKEN\" ;;\n"
                "esac\n",
                encoding="utf-8",
            )
            askpass.chmod(0o700)
            push_environment = dict(environment)
            push_environment.update(
                {
                    "GIT_ASKPASS": str(askpass),
                    "GIT_TERMINAL_PROMPT": "0",
                    "SWARM_GITHUB_APP_PUSH_TOKEN": environment["GH_TOKEN"],
                }
            )
            run_command(
                [
                    self.config.git_bin,
                    "-C",
                    self.config.repo_dir,
                    "push",
                    f"https://{self.config.github_host}/{self.config.github_repository}.git",
                    f"HEAD:refs/heads/{branch}",
                ],
                env=push_environment,
            )
        if existing and existing[0].get("state") == "OPEN":
            pr_url = str(existing[0]["url"])
            log(f"Reusing existing pull request {pr_url} for issue #{self.issue.number}.")
        else:
            title = self.git("log", "-1", "--format=%s", commit_sha)
            body = (
                f"Automated {self.choice.name} implementation for #{self.issue.number}.\n\n"
                f"Commit: `{commit_sha}`\n"
            )
            output = self.github.gh(
                [
                    "pr",
                    "create",
                    "--repo",
                    self.config.github_repository,
                    "--head",
                    branch,
                    "--base",
                    self.config.base_branch,
                    "--title",
                    title,
                    "--body-file",
                    "-",
                ],
                self.choice.key,
                body,
            ).strip()
            pr_url = output.splitlines()[-1]
        delivered_sha = commit_sha
        if self.config.auto_approve:
            self.approve_pull_request(pr_url)
        if self.config.auto_merge:
            self.github.gh(
                [
                    "pr",
                    "merge",
                    pr_url,
                    "--repo",
                    self.config.github_repository,
                    f"--{self.config.merge_method}",
                    "--delete-branch",
                    "--match-head-commit",
                    commit_sha,
                ],
                self.choice.key,
            )
            delivered_sha = self.github.gh(
                [
                    "pr",
                    "view",
                    pr_url,
                    "--repo",
                    self.config.github_repository,
                    "--json",
                    "mergeCommit",
                    "--jq",
                    ".mergeCommit.oid",
                ],
                self.choice.key,
            ).strip()
            if not SHA_RE.fullmatch(delivered_sha):
                raise WorkerError(f"Merged PR did not report a valid merge commit: {pr_url}")
            # Fetch through normal user credentials; the local repo already has
            # a configured authenticated origin and this avoids token persistence.
            self.return_to_synchronized_main(branch)
        return pr_url, branch, delivered_sha

    def finalize_issue(self, commit_sha: str, ai_output: str) -> None:
        assert self.issue and self.choice
        pr_url = ""
        branch = ""
        if self.config.delivery_mode == "pull-request":
            pr_url, branch, commit_sha = self.deliver_pull_request(commit_sha)
        pending = {
            "issue_number": self.issue.number,
            "issue_title": self.issue.title,
            "issue_url": self.issue.url,
            "ai": self.choice.name,
            "ai_tool": self.choice.name,
            "model": self.choice.model,
            "effort": self.choice.effort,
            "work_type": self.issue.work_type,
            "trigger_comment_id": self.issue.trigger_comment_id,
            "ai_output": ai_output,
            "commit_sha": commit_sha,
            "commit_message": self.git("log", "-1", "--format=%s", commit_sha),
            "github_comment_posted": False,
            "ready_for_testing_label_added": False,
            "pull_request_url": pr_url,
            "branch_name": branch,
        }
        atomic_write_json(self.pending_file, pending)
        pending = self.post_pending_comment(pending)
        pending = self.add_pending_label(pending)
        log(f"Sending notification for issue #{self.issue.number}.")
        self.send_notification(pending)
        self.record_completed(self.issue.number)
        self.pending_file.unlink()
        self.clear_in_progress(self.issue.number)
        log(
            f"Finished issue #{self.issue.number} with {self.choice.name}: "
            f"{pending['commit_message']} ({commit_sha})."
        )

    def run_selected_issue(self) -> int:
        assert self.issue
        if self.issue.work_type == "followup":
            log(
                f"Selected issue #{self.issue.number} for rework after GitHub follow-up comment "
                f"{self.issue.trigger_comment_id}: {self.issue.title}"
            )
        else:
            log(f"Selected oldest unprocessed assigned issue: #{self.issue.number} {self.issue.title}")

        if self.in_progress_file.exists():
            state = self.read_state()
            self.choice = self.choice_from_state(state)
            if not self.quota_resume_ready:
                capacity = self.provider_capacity(self.choice.name)
                if capacity == 2:
                    log(
                        f"Could not verify {self.choice.name} usage for pinned issue #{self.issue.number}; "
                        "leaving state active and retrying later."
                    )
                    return 0
                if capacity == 1:
                    if self.config.dry_run:
                        log(
                            f"Dry run: pinned {self.choice.name} session {self.choice.session_id} is waiting for usage."
                        )
                        return 0
                    if not self.choice.session_id:
                        raise WorkerError(f"Pinned {self.choice.name} attempt has no resumable session ID")
                    self.mark_quota_paused()
                    self.deliver_quota_notifications()
                    self.suspend_paused()
                    return QUOTA_PAUSED_EXIT_CODE
        else:
            availability = {
                "Claude": self.claude_capacity() == 0,
                "Codex": self.codex_capacity() == 0,
            }
            self.choice = self.choose_provider(self.issue.previous_ai, availability)
            if not self.choice:
                log(
                    f"Neither Claude nor Codex has at least {self.config.minimum_remaining_percent:g}% "
                    "remaining in every active quota window; stopping."
                )
                return 0

        assert self.choice
        if self.choice.resume:
            log(
                f"Pinned {self.choice.name} model {self.choice.model} session {self.choice.session_id} "
                f"with effort {self.choice.effort} for this continuation."
            )
        else:
            log(f"Selected {self.choice.name} model {self.choice.model} with effort {self.choice.effort} for this run.")
        if self.config.require_bot_auth and not self.apps.configured(self.choice.key):
            raise WorkerError(
                f"Bot auth is required, but {self.choice.name} has no entry in {self.config.github_apps_config}"
            )
        if self.config.dry_run:
            log(f"Dry run complete: would run {self.choice.name} for {self.issue.url}.")
            return 0

        run_start, recovery_mode, candidate, recovery_dirty = self.prepare_repository()
        self.post_started_comment()
        prompt = self.build_prompt(recovery_mode, candidate, recovery_dirty)
        ai_status = self.run_ai(prompt)
        if ai_status != 0 or not self.ai_output_file.exists() or self.ai_output_file.stat().st_size == 0:
            if self.ai_failure_is_quota():
                if not self.choice.session_id:
                    raise WorkerError(
                        f"{self.choice.name} exhausted usage before returning a resumable session ID; "
                        "worktree preserved but automatic resume is unavailable"
                    )
                self.mark_quota_paused()
                self.deliver_quota_notifications()
                self.suspend_paused()
                return QUOTA_PAUSED_EXIT_CODE
            if ai_status != 0:
                raise WorkerError(
                    f"{self.choice.name} exited unsuccessfully. Session and repository state were preserved; "
                    f"see {self.ai_output_file} and {self.ai_diagnostic_file}."
                )
            raise WorkerError(
                f"{self.choice.name} finished without a final summary. Session and repository state were preserved."
            )

        output = self.ai_output_file.read_text(encoding="utf-8", errors="replace")
        if self.choice.name == "Codex":
            print("\n--- Codex completion summary ---")
            print(output, end="" if output.endswith("\n") else "\n")
        if self.git("branch", "--show-current") != self.expected_branch():
            raise WorkerError(
                f"{self.choice.name} changed branches; refusing to commit outside {self.expected_branch()}"
            )
        after = self.commit_completed_work(run_start)
        completion = after
        recovered = False
        if after != run_start:
            pass
        elif recovery_mode and candidate and re.search(r"^\s*SWARM_RECOVERY_COMPLETE\s*$", output, re.MULTILINE):
            completion = candidate
            recovered = True
            output = re.sub(r"^\s*SWARM_RECOVERY_COMPLETE\s*$\n?", "", output, flags=re.MULTILINE)
            self.ai_output_file.write_text(output, encoding="utf-8")
            log(f"Accepted commit {completion} as recovered implementation for issue #{self.issue.number}.")
        else:
            raise WorkerError(
                f"{self.choice.name} finished without producing changes or a new commit. "
                "Recovery state was preserved."
            )
        if self.git("branch", "--show-current") != self.expected_branch():
            raise WorkerError(f"{self.choice.name} changed branches; commit was not left on {self.expected_branch()}")
        base = str(self.read_state()["base_sha"])
        if not self.git_ok("merge-base", "--is-ancestor", base, after):
            raise WorkerError(f"{self.choice.name} rewrote history instead of adding a descendant commit")
        completion = self.ensure_issue_reference(completion, recovered)
        if self.git("status", "--porcelain"):
            raise WorkerError(
                f"Issue #{self.issue.number} cannot be delivered with uncommitted changes"
            )
        self.finalize_issue(completion, output)
        return ISSUE_COMPLETED_EXIT_CODE

    def run(self) -> int:
        with PidLock(self.lock_dir, "worker"):
            for executable, label in (
                (self.config.gh_bin, "gh"),
                (self.config.git_bin, "git"),
                (self.config.python_bin, "python3"),
            ):
                if not command_available(executable):
                    raise WorkerError(f"{label} is required but was not found in PATH")
            self.deliver_pending()
            if self.prepare_paused_resume():
                return 0
            self.issue = self.select_issue()
            if not self.issue:
                return 0
            return self.run_selected_issue()


def executable_default(name: str) -> str:
    return shutil.which(name) or ""


def build_parser() -> argparse.ArgumentParser:
    script_dir = SCRIPT_HOME
    home = Path.home()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-dir", default=env_value("SWARM_REPO_DIR", str(script_dir.parent.parent)))
    parser.add_argument(
        "--state-dir",
        default=env_value("SWARM_ISSUE_WORKER_STATE_DIR", str(home / ".local/state/swarm-issue-worker")),
    )
    parser.add_argument(
        "--github-repository", default=env_value("SWARM_GITHUB_REPOSITORY", "DotNetRockStar/swarm")
    )
    parser.add_argument("--assignee", default=env_value("SWARM_GITHUB_ASSIGNEE", "DotNetRockStar"))
    parser.add_argument(
        "--trusted-followup-author",
        action="append",
        default=list(csv_values(env_value("SWARM_TRUSTED_FOLLOWUP_AUTHORS", env_value("SWARM_TRUSTED_FOLLOWUP_AUTHOR", "DotNetRockStar")))),
    )
    parser.add_argument(
        "--completion-author",
        action="append",
        default=list(csv_values(env_value("SWARM_COMPLETION_AUTHORS", "DotNetRockStar"))),
    )
    parser.add_argument(
        "--ready-label", default=env_value("SWARM_READY_FOR_TESTING_LABEL", "Ready For Testing")
    )
    parser.add_argument(
        "--minimum-remaining-percent",
        type=float,
        default=float(env_value("SWARM_MIN_REMAINING_PERCENT", "10")),
    )
    parser.add_argument("--claude-model", default=env_value("SWARM_CLAUDE_MODEL", "claude-sonnet-5"))
    parser.add_argument("--codex-model", default=env_value("SWARM_CODEX_MODEL", "gpt-5.6-sol"))
    parser.add_argument("--claude-effort", default=env_value("SWARM_CLAUDE_EFFORT", "high"))
    parser.add_argument("--codex-effort", default=env_value("SWARM_CODEX_EFFORT", "high"))
    parser.add_argument(
        "--preferred-provider",
        choices=("claude", "codex"),
        default=env_value("SWARM_PREFERRED_PROVIDER", "claude").lower(),
    )
    parser.add_argument("--email-to", default=env_value("SWARM_EMAIL_TO", "mr_jerrodh@hotmail.com"))
    parser.add_argument("--smtp-credentials-file", default=env_value("SWARM_SMTP_CREDENTIALS_FILE", ""))
    parser.add_argument("--no-email", action="store_true", default=env_bool("SWARM_NO_EMAIL", False))
    parser.add_argument("--dry-run", action="store_true", default=env_bool("SWARM_ISSUE_WORKER_DRY_RUN"))
    parser.add_argument("--gh-bin", default=env_value("GH_BIN", executable_default("gh")))
    parser.add_argument("--git-bin", default=env_value("GIT_BIN", executable_default("git")))
    parser.add_argument("--python-bin", default=env_value("PYTHON_BIN", executable_default("python3")))
    parser.add_argument("--claude-bin", default=env_value("CLAUDE_BIN", executable_default("claude")))
    parser.add_argument("--codex-bin", default=env_value("CODEX_BIN", executable_default("codex")))
    parser.add_argument(
        "--github-apps-config",
        default=env_value("SWARM_GITHUB_APPS_CONFIG", str(DEFAULT_CONFIG_PATH)),
    )
    parser.add_argument("--openssl-bin", default=env_value("OPENSSL_BIN", executable_default("openssl") or "openssl"))
    parser.add_argument(
        "--require-bot-auth",
        action=argparse.BooleanOptionalAction,
        default=env_bool("SWARM_REQUIRE_BOT_AUTH", True),
    )
    parser.add_argument(
        "--delivery-mode",
        choices=("local-main", "pull-request"),
        default=env_value("SWARM_DELIVERY_MODE", "pull-request"),
    )
    parser.add_argument(
        "--auto-approve",
        action=argparse.BooleanOptionalAction,
        default=env_bool("SWARM_AUTO_APPROVE", True),
    )
    parser.add_argument(
        "--auto-merge",
        action=argparse.BooleanOptionalAction,
        default=env_bool("SWARM_AUTO_MERGE", True),
    )
    parser.add_argument("--branch-prefix", default=env_value("SWARM_BRANCH_PREFIX", "swarm"))
    parser.add_argument("--base-branch", default=env_value("SWARM_BASE_BRANCH", "main"))
    parser.add_argument("--remote-name", default=env_value("SWARM_GIT_REMOTE", "origin"))
    parser.add_argument(
        "--merge-method",
        choices=("merge", "squash", "rebase"),
        default=env_value("SWARM_PR_MERGE_METHOD", "merge"),
    )
    parser.add_argument("--github-host", default=env_value("SWARM_GITHUB_HOST", "github.com"))
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if not 0 <= args.minimum_remaining_percent <= 100:
        raise WorkerError("--minimum-remaining-percent must be between 0 and 100")
    if args.auto_merge and args.delivery_mode != "pull-request":
        raise WorkerError("--auto-merge requires --delivery-mode pull-request")
    if args.auto_approve and args.delivery_mode != "pull-request":
        raise WorkerError("--auto-approve requires --delivery-mode pull-request")
    config = Config.from_args(args)
    return Worker(config).run()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
    except (WorkerError, OSError, ValueError, json.JSONDecodeError) as error:
        log(f"ERROR: {error}")
        raise SystemExit(1)
