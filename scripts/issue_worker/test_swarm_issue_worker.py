#!/usr/bin/env python3

from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import github_app_auth as auth_module
import install_swarm_issue_cron as runner_module
import setup_github_bots as setup_module
from swarm_issue_worker import (
    Config,
    IssueContext,
    ProviderChoice,
    Worker,
    build_parser,
    extract_completion_metadata,
    extract_followup_metadata,
)


class WorkerTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="swarm-worker-test.")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.state = self.root / "state"
        self.repo.mkdir()
        self.state.mkdir()
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "SWARM worker test")
        self.git("config", "user.email", "worker-test@example.invalid")
        (self.repo / "tracked.txt").write_text("base\n", encoding="utf-8")
        self.git("add", "tracked.txt")
        self.git("commit", "-q", "-m", "base")
        self.base_sha = self.git("rev-parse", "HEAD")
        args = build_parser().parse_args(
            [
                "--repo-dir", str(self.repo), "--state-dir", str(self.state), "--no-email",
                "--gh-bin", "/usr/bin/false", "--claude-bin", "", "--codex-bin", "",
            ]
        )
        self.worker = Worker(Config.from_args(args))

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def git(self, *args: str) -> str:
        return subprocess.run(
            ["git", "-C", str(self.repo), *args], text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True,
        ).stdout.strip()

    def paused_state(self, issue_number: int = 101) -> dict[str, object]:
        return {
            "issue_number": issue_number, "issue_title": "Paused work",
            "issue_url": f"https://example.invalid/issues/{issue_number}", "base_sha": self.base_sha,
            "work_type": "initial", "previous_commit_sha": "", "previous_completion_comment": None,
            "followup_comments": [], "trigger_comment_id": None, "ai_tool": "Claude",
            "model": "test-model", "effort": "high", "session_id": f"session-{issue_number}",
            "session_comment_id": 0, "status": "quota_paused", "quota_pause_count": 1,
            "quota_paused_at": "2026-08-25T10:00:00-05:00", "attempt_start_sha": self.base_sha,
        }

    def test_followup_accepts_human_and_bot_completion_authors(self) -> None:
        comments = [
            {"id": 100, "created_at": "2026-08-25T10:00:00Z", "user": {"login": "swarm-codex-bot[bot]"},
             "body": "<!-- swarm-issue-worker:commit:" + "1" * 40 + " -->\nCompleted by **Codex**."},
            {"id": 101, "created_at": "2026-08-25T10:01:00Z", "user": {"login": "swarm-codex-bot[bot]"},
             "body": "<!-- swarm-issue-worker:quota-paused:issue:71;pause:1;session:test -->\nWork paused."},
            {"id": 102, "created_at": "2026-08-25T10:02:00Z", "user": {"login": "DotNetRockStar"},
             "body": "Please add a disk-usage graph."},
            {"id": 103, "created_at": "2026-08-25T10:03:00Z", "user": {"login": "someone-else"},
             "body": "Untrusted request."},
        ]
        completion_authors = {"DotNetRockStar", "swarm-codex-bot[bot]"}
        followup = extract_followup_metadata(comments, {"DotNetRockStar"}, completion_authors)
        assert followup is not None
        self.assertEqual(followup["trigger_comment_id"], 102)
        self.assertEqual(len(followup["followup_comments"]), 1)
        self.assertEqual(followup["previous_ai"], "Codex")
        completion = extract_completion_metadata(comments, completion_authors)
        assert completion is not None
        self.assertEqual(completion["commit_sha"], "1" * 40)
        self.assertIsNone(extract_completion_metadata(comments, {"someone-else"}))

    def test_followup_prefers_opposite_provider(self) -> None:
        choice = self.worker.choose_provider("Claude", {"Claude": True, "Codex": True})
        assert choice is not None
        self.assertEqual(choice.name, "Codex")
        choice = self.worker.choose_provider("Codex", {"Claude": True, "Codex": True})
        assert choice is not None
        self.assertEqual(choice.name, "Claude")
        self.assertTrue(choice.session_id)

    def test_pause_shelves_and_restore_preserves_newer_commit(self) -> None:
        self.worker.write_state(self.paused_state())
        (self.repo / "tracked.txt").write_text("base\npaused change\n", encoding="utf-8")
        (self.repo / "untracked.txt").write_text("untracked change\n", encoding="utf-8")
        self.worker.suspend_paused()
        paused_file = self.worker.paused_dir / "101.json"
        self.assertTrue(paused_file.is_file())
        self.assertFalse(self.worker.in_progress_file.exists())
        self.assertEqual(self.git("status", "--porcelain"), "")
        (self.repo / "other.txt").write_text("other issue\n", encoding="utf-8")
        self.git("add", "other.txt")
        self.git("commit", "-q", "-m", "other issue")
        newer_sha = self.git("rev-parse", "HEAD")
        self.worker.restore_paused(paused_file)
        self.assertEqual(self.git("rev-parse", "HEAD"), newer_sha)
        self.assertIn("paused change", (self.repo / "tracked.txt").read_text())
        self.assertEqual((self.repo / "untracked.txt").read_text(), "untracked change\n")

    def test_damaged_recovery_sha_is_repaired_by_unique_prefix(self) -> None:
        (self.repo / "tracked.txt").write_text("base\nissue work\n", encoding="utf-8")
        self.git("add", "tracked.txt")
        self.git("commit", "-q", "-m", "issue work #202")
        candidate = self.git("rev-parse", "HEAD")
        state = self.paused_state(202)
        state["ai_tool"] = "Codex"
        state["candidate_sha"] = candidate[:8] + "0" * 32
        state["attempt_start_sha"] = candidate
        self.worker.write_state(state)
        normalized = self.worker.normalize_recovery_commits(self.worker.in_progress_file, candidate)
        self.assertEqual(normalized["candidate_sha"], candidate)

    def test_recovery_records_existing_candidate_with_dirty_worktree(self) -> None:
        self.worker.issue = IssueContext(303, "Recovery", "", [], "https://example.invalid/303")
        self.worker.choice = ProviderChoice("Codex", "test", "high", "session", True)
        self.worker.save_new_state(self.worker.issue, self.worker.choice, self.base_sha)
        (self.repo / "tracked.txt").write_text("base\nfixed\n", encoding="utf-8")
        self.git("add", "tracked.txt")
        self.git("commit", "-q", "-m", "complete issue #303")
        (self.repo / "unrelated.txt").write_text("unrelated\n", encoding="utf-8")
        run_start, recovery, candidate, dirty = self.worker.prepare_repository()
        self.assertTrue(recovery)
        self.assertTrue(dirty)
        self.assertEqual(candidate, run_start)
        self.assertEqual(self.worker.read_state()["candidate_sha"], run_start)

    def test_completion_markdown_is_not_indented(self) -> None:
        pending = {
            "ai": "Codex", "ai_tool": "Codex", "model": "test-model", "effort": "high",
            "commit_sha": "1" * 40, "commit_message": "Test completion (#404)",
            "ai_output": "## Summary\n\nDone.\n\n## Changes\n\n- Fixed it.", "work_type": "initial",
        }
        rendered = self.worker.render_pending_comment(pending)
        self.assertIn("\n## Summary\n", rendered)
        self.assertIn("\n- Fixed it.\n", rendered)
        self.assertNotIn("    ## Summary", rendered)

    def test_defaults_and_parameters(self) -> None:
        args = build_parser().parse_args([])
        self.assertEqual(args.github_repository, "DotNetRockStar/swarm")
        self.assertEqual(args.assignee, "DotNetRockStar")
        self.assertEqual(args.minimum_remaining_percent, 10)
        self.assertEqual(args.delivery_mode, "local-main")
        overridden = build_parser().parse_args(
            ["--github-repository", "example/repo", "--preferred-provider", "codex", "--delivery-mode", "pull-request"]
        )
        self.assertEqual(overridden.github_repository, "example/repo")
        self.assertEqual(overridden.preferred_provider, "codex")


class RunnerTestCase(unittest.TestCase):
    def test_active_transcode_diagnostic_and_runner_lock(self) -> None:
        with tempfile.TemporaryDirectory(prefix="swarm-runner-test.") as temporary:
            root = Path(temporary)
            pgrep = root / "pgrep"
            pgrep.write_text("#!/bin/sh\nexit \"${FAKE_PGREP_STATUS:-1}\"\n", encoding="utf-8")
            pgrep.chmod(0o755)
            args = runner_module.build_parser().parse_args(
                ["--state-dir", str(root / "state"), "--pgrep-bin", str(pgrep), "--check-transcode-active"]
            )
            with mock.patch.dict(os.environ, {"FAKE_PGREP_STATUS": "0"}):
                self.assertEqual(runner_module.Runner(args, []).run(), 0)
            args.check_transcode_active = False
            lock = Path(args.state_dir) / "runner.lock"
            lock.mkdir(parents=True)
            (lock / "pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(runner_module.Runner(args, []).run(), 0)
            self.assertIn("already active", output.getvalue())

    def test_scheduler_snapshot_forwards_shared_paths(self) -> None:
        with tempfile.TemporaryDirectory(prefix="swarm-runner-snapshot-test.") as temporary:
            root = Path(temporary)
            repo = root / "repo"
            state = root / "state"
            repo.mkdir()
            worker = root / "fake_worker.py"
            result_file = root / "result.json"
            worker.write_text(
                "import json, os, sys\n"
                "from pathlib import Path\n"
                "Path(os.environ['FAKE_RESULT_FILE']).write_text(json.dumps({"
                "'repo': os.environ.get('SWARM_REPO_DIR'), "
                "'state': os.environ.get('SWARM_ISSUE_WORKER_STATE_DIR'), "
                "'args': sys.argv[1:]}))\n",
                encoding="utf-8",
            )
            args = runner_module.build_parser().parse_args(
                [
                    "--repo-dir", str(repo), "--state-dir", str(state), "--worker", str(worker),
                    "--once", "--no-email", "--pgrep-bin", "",
                ]
            )
            with mock.patch.dict(os.environ, {"FAKE_RESULT_FILE": str(result_file)}):
                self.assertEqual(runner_module.Runner(args, ["--github-repository", "example/repo"]).run(), 0)
            result = json.loads(result_file.read_text(encoding="utf-8"))
            self.assertEqual(result["repo"], str(repo.resolve()))
            self.assertEqual(result["state"], str(state.resolve()))
            self.assertEqual(result["args"], ["--github-repository", "example/repo", "--no-email"])


class GitHubAppAuthTestCase(unittest.TestCase):
    def test_token_and_bot_identity_are_short_lived_and_not_persisted(self) -> None:
        with tempfile.TemporaryDirectory(prefix="swarm-app-auth-test.") as temporary:
            root = Path(temporary)
            key = root / "bot.pem"
            subprocess.run(["openssl", "genrsa", "-out", str(key), "2048"], check=True, capture_output=True)
            key.chmod(0o600)
            config = root / "apps.json"
            config.write_text(
                json.dumps(
                    {
                        "codex": {
                            "app_id": 123,
                            "installation_id": 456,
                            "private_key_path": str(key),
                            "bot_login": "swarm-codex-bot[bot]",
                            "bot_name": "Swarm Codex Bot",
                        }
                    }
                ),
                encoding="utf-8",
            )
            responses = [
                io.BytesIO(json.dumps({"token": "installation-token"}).encode()),
                io.BytesIO(json.dumps({"id": 789}).encode()),
            ]
            with mock.patch.object(auth_module.urllib.request, "urlopen", side_effect=responses) as urlopen:
                auth = auth_module.GitHubAppAuth(config)
                environment = auth.bot_environment("codex")
                self.assertEqual(environment["GH_TOKEN"], "installation-token")
                self.assertEqual(
                    environment["GIT_AUTHOR_EMAIL"],
                    "789+swarm-codex-bot[bot]@users.noreply.github.com",
                )
                self.assertEqual(urlopen.call_count, 2)
            self.assertNotIn("installation-token", config.read_text(encoding="utf-8"))

    def test_private_key_must_not_be_group_readable(self) -> None:
        with tempfile.TemporaryDirectory(prefix="swarm-app-key-test.") as temporary:
            root = Path(temporary)
            key = root / "bot.pem"
            key.write_text("not used", encoding="utf-8")
            key.chmod(0o644)
            config = root / "apps.json"
            config.write_text(
                json.dumps(
                    {
                        "claude": {
                            "app_id": 1,
                            "installation_id": 2,
                            "private_key_path": str(key),
                            "bot_login": "swarm-claude-bot[bot]",
                        }
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(RuntimeError, "permissions are too broad"):
                auth_module.GitHubAppAuth(config).definition("claude")

    def test_setup_manifest_is_private_and_minimally_scoped(self) -> None:
        with tempfile.TemporaryDirectory(prefix="swarm-app-manifest-test.") as temporary:
            state = setup_module.SetupState(
                "DotNetRockStar/swarm", Path(temporary) / "apps.json", 8765
            )
            manifest = state.manifest("codex")
            self.assertFalse(manifest["public"])
            self.assertNotIn("hook_attributes", manifest)
            self.assertEqual(
                manifest["default_permissions"],
                {
                    "contents": "write",
                    "issues": "write",
                    "pull_requests": "write",
                    "workflows": "write",
                },
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
