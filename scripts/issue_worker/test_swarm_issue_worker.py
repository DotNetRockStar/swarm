#!/usr/bin/env python3

from __future__ import annotations

import contextlib
import datetime as dt
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
    is_worker_comment,
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
        self.remote = self.root / "remote.git"
        subprocess.run(["git", "init", "-q", "--bare", str(self.remote)], check=True)
        self.git("remote", "add", "origin", str(self.remote))
        self.git("push", "-q", "-u", "origin", "main")
        args = build_parser().parse_args(
            [
                "--repo-dir", str(self.repo), "--state-dir", str(self.state), "--no-email",
                "--gh-bin", "/usr/bin/false", "--claude-bin", "", "--codex-bin", "",
                "--delivery-mode", "local-main", "--no-auto-approve", "--no-auto-merge",
                "--no-require-bot-auth",
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

    def pr_worker(self) -> Worker:
        args = build_parser().parse_args(
            [
                "--repo-dir", str(self.repo), "--state-dir", str(self.state), "--no-email",
                "--gh-bin", "/usr/bin/false", "--claude-bin", "", "--codex-bin", "",
                "--delivery-mode", "pull-request", "--auto-approve", "--auto-merge",
                "--no-require-bot-auth",
            ]
        )
        return Worker(Config.from_args(args))

    def paused_state(self, issue_number: int = 101) -> dict[str, object]:
        return {
            "issue_number": issue_number, "issue_title": "Paused work",
            "issue_url": f"https://example.invalid/issues/{issue_number}", "base_sha": self.base_sha,
            "work_type": "initial", "previous_commit_sha": "", "previous_completion_comment": None,
            "followup_comments": [], "trigger_comment_id": None, "ai_tool": "Claude",
            "model": "test-model", "effort": "high", "session_id": f"session-{issue_number}",
            "session_started": True,
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

    def test_choice_from_state_does_not_resume_a_session_that_never_started(self) -> None:
        # prepare_repository persists a freshly-generated Claude session_id
        # before run_ai ever invokes `claude` with it (e.g. so a retry after
        # post_started_comment fails still knows which ID to assign). A
        # crash in that window must not make the next attempt think there's
        # a real session to --resume — regression test for that exact bug.
        state = {
            "ai_tool": "Claude", "model": "test-model", "effort": "high",
            "session_id": "never-actually-started",
        }
        choice = self.worker.choice_from_state(state)
        self.assertEqual(choice.session_id, "never-actually-started")
        self.assertFalse(choice.resume)

    def test_choice_from_state_resumes_once_session_started_is_recorded(self) -> None:
        state = {
            "ai_tool": "Claude", "model": "test-model", "effort": "high",
            "session_id": "genuinely-running", "session_started": True,
        }
        choice = self.worker.choice_from_state(state)
        self.assertTrue(choice.resume)

    @staticmethod
    def issue_payload(number: int) -> dict[str, object]:
        return {
            "number": number,
            "title": f"Issue {number}",
            "body": "",
            "labels": [],
            "assignees": [{"login": "DotNetRockStar"}],
            "html_url": f"https://example.invalid/{number}",
            "created_at": f"2026-08-{number % 28 + 1:02d}T00:00:00Z",
        }

    def test_assigned_issues_are_sorted_by_number_not_api_or_timestamp_order(self) -> None:
        issues = [self.issue_payload(55), self.issue_payload(50), self.issue_payload(53)]
        with mock.patch.object(self.worker.github, "api_list", return_value=issues):
            selected = self.worker.assigned_issues()
        self.assertEqual([int(issue["number"]) for issue in selected], [50, 53, 55])

    def test_lower_fresh_issue_beats_higher_followup_issue(self) -> None:
        self.worker.completed_file.write_text("55\n", encoding="utf-8")
        issues = [self.issue_payload(55), self.issue_payload(50)]
        followup_comments = [
            {
                "id": 100,
                "created_at": "2026-08-20T00:00:00Z",
                "user": {"login": "DotNetRockStar"},
                "body": "<!-- swarm-issue-worker:commit:" + "1" * 40 + " -->\nCompleted by **Codex**.",
            },
            {
                "id": 101,
                "created_at": "2026-08-21T00:00:00Z",
                "user": {"login": "DotNetRockStar"},
                "body": "Please revisit this.",
            },
        ]
        with (
            mock.patch.object(self.worker, "assigned_issues", return_value=issues),
            mock.patch.object(
                self.worker,
                "comments",
                side_effect=lambda number: followup_comments if number == 55 else [],
            ),
        ):
            selected = self.worker.select_issue()
        assert selected is not None
        self.assertEqual(selected.number, 50)
        self.assertEqual(selected.work_type, "initial")

    def test_lower_followup_issue_beats_higher_fresh_issue(self) -> None:
        self.worker.completed_file.write_text("50\n", encoding="utf-8")
        issues = [self.issue_payload(55), self.issue_payload(50)]
        followup_comments = [
            {
                "id": 200,
                "created_at": "2026-08-20T00:00:00Z",
                "user": {"login": "DotNetRockStar"},
                "body": "<!-- swarm-issue-worker:commit:" + "2" * 40 + " -->\nCompleted by **Claude**.",
            },
            {
                "id": 201,
                "created_at": "2026-08-21T00:00:00Z",
                "user": {"login": "DotNetRockStar"},
                "body": "Please revisit this first.",
            },
        ]
        with (
            mock.patch.object(self.worker, "assigned_issues", return_value=issues),
            mock.patch.object(
                self.worker,
                "comments",
                side_effect=lambda number: followup_comments if number == 50 else [],
            ),
        ):
            selected = self.worker.select_issue()
        assert selected is not None
        self.assertEqual(selected.number, 50)
        self.assertEqual(selected.work_type, "followup")

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

    def test_closed_shelved_pause_is_archived_without_ai_or_quota_check(self) -> None:
        self.worker.paused_dir.mkdir()
        paused_file = self.worker.paused_dir / "101.json"
        self.worker.write_state(self.paused_state(), paused_file)

        with (
            mock.patch.object(self.worker, "issue_is_closed", return_value=True),
            mock.patch.object(self.worker, "provider_capacity") as capacity,
            mock.patch.object(self.worker, "restore_paused") as restore,
        ):
            self.assertFalse(self.worker.prepare_paused_resume())

        capacity.assert_not_called()
        restore.assert_not_called()
        self.assertFalse(paused_file.exists())
        archive = self.worker.closed_paused_dir / "101.json"
        self.assertTrue(archive.is_file())
        archived_state = self.worker.read_state(archive)
        self.assertEqual(archived_state["status"], "closed_while_paused")
        self.assertEqual(archived_state["archived_from"], "101.json")

    def test_closed_in_progress_pause_shelves_work_and_returns_to_main_without_ai(self) -> None:
        worker = self.pr_worker()
        worker.issue = IssueContext(102, "Closed pause", "", [], "https://example.invalid/102")
        worker.choice = ProviderChoice("Claude", "test", "high", "session-102")
        worker.prepare_repository()
        worker.update_state(
            status="quota_paused",
            quota_pause_count=1,
            quota_paused_at="2026-08-25T10:00:00-05:00",
        )
        (self.repo / "paused.txt").write_text("preserve this work\n", encoding="utf-8")

        with (
            mock.patch.object(worker, "issue_is_closed", return_value=True),
            mock.patch.object(worker, "deliver_quota_notifications") as notifications,
            mock.patch.object(worker, "provider_capacity") as capacity,
        ):
            self.assertFalse(worker.prepare_paused_resume())

        notifications.assert_not_called()
        capacity.assert_not_called()
        self.assertEqual(self.git("branch", "--show-current"), "main")
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertFalse(worker.in_progress_file.exists())
        archive = worker.closed_paused_dir / "102.json"
        archived_state = worker.read_state(archive)
        self.assertEqual(archived_state["status"], "closed_while_paused")
        self.assertTrue(archived_state["worktree_stash_oid"])
        self.assertTrue(self.git("cat-file", "-e", archived_state["worktree_stash_oid"] + "^{commit}") == "")

    def test_closed_pause_does_not_prevent_fresh_selection_after_reopen(self) -> None:
        self.worker.paused_dir.mkdir()
        paused_file = self.worker.paused_dir / "103.json"
        self.worker.write_state(self.paused_state(103), paused_file)
        with mock.patch.object(self.worker, "issue_is_closed", return_value=True):
            self.assertFalse(self.worker.prepare_paused_resume())

        issue = self.issue_payload(103)
        with (
            mock.patch.object(self.worker, "assigned_issues", return_value=[issue]),
            mock.patch.object(self.worker, "comments", return_value=[]),
        ):
            selected = self.worker.select_issue()
        assert selected is not None
        self.assertEqual(selected.number, 103)
        self.assertEqual(selected.work_type, "initial")

    def test_issue_is_closed_reads_current_github_state(self) -> None:
        with mock.patch.object(
            self.worker.github, "gh", return_value=json.dumps({"number": 104, "state": "closed"})
        ) as gh:
            self.assertTrue(self.worker.issue_is_closed(104))
        gh.assert_called_once_with(
            ["api", "--method", "GET", "repos/DotNetRockStar/swarm/issues/104"]
        )

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

    def test_pr_state_exists_before_branch_creation_and_recreates_interrupted_branch(self) -> None:
        worker = self.pr_worker()
        worker.issue = IssueContext(401, "Transactional branch", "", [], "https://example.invalid/401")
        worker.choice = ProviderChoice("Codex", "test", "high", "session")
        original_git = worker.git

        def interrupt_branch_creation(*arguments: str, **kwargs: object) -> str:
            if arguments[:2] == ("switch", "-c"):
                raise RuntimeError("simulated interruption")
            return original_git(*arguments, **kwargs)

        with mock.patch.object(worker, "git", side_effect=interrupt_branch_creation):
            with self.assertRaisesRegex(RuntimeError, "simulated interruption"):
                worker.prepare_repository()
        state = worker.read_state()
        self.assertEqual(state["branch_name"], "swarm/codex/issue-401")
        self.assertEqual(state["base_sha"], self.git("rev-parse", "main"))
        self.assertEqual(self.git("branch", "--show-current"), "main")

        run_start, recovery, candidate, dirty = worker.prepare_repository()
        self.assertTrue(recovery)
        self.assertFalse(candidate)
        self.assertFalse(dirty)
        self.assertEqual(run_start, state["base_sha"])
        self.assertEqual(self.git("branch", "--show-current"), "swarm/codex/issue-401")

    def test_fresh_pr_branch_fast_forwards_main_before_branching(self) -> None:
        updater = self.root / "updater"
        subprocess.run(
            ["git", "clone", "-q", "--branch", "main", str(self.remote), str(updater)], check=True
        )
        subprocess.run(["git", "-C", str(updater), "config", "user.name", "updater"], check=True)
        subprocess.run(
            ["git", "-C", str(updater), "config", "user.email", "updater@example.invalid"], check=True
        )
        (updater / "remote.txt").write_text("new main work\n", encoding="utf-8")
        subprocess.run(["git", "-C", str(updater), "add", "remote.txt"], check=True)
        subprocess.run(["git", "-C", str(updater), "commit", "-q", "-m", "remote update"], check=True)
        subprocess.run(["git", "-C", str(updater), "push", "-q", "origin", "main"], check=True)
        remote_main = subprocess.run(
            ["git", "-C", str(updater), "rev-parse", "HEAD"], text=True,
            stdout=subprocess.PIPE, check=True,
        ).stdout.strip()

        worker = self.pr_worker()
        worker.issue = IssueContext(402, "Fresh base", "", [], "https://example.invalid/402")
        worker.choice = ProviderChoice("Claude", "test", "high", "session")
        run_start, recovery, _, _ = worker.prepare_repository()
        self.assertFalse(recovery)
        self.assertEqual(run_start, remote_main)
        self.assertEqual(self.git("rev-parse", "main"), remote_main)
        self.assertEqual(self.git("branch", "--show-current"), "swarm/claude/issue-402")

    def test_worker_commits_uncommitted_completed_work(self) -> None:
        worker = self.pr_worker()
        worker.issue = IssueContext(403, "Commit completed files", "", [], "https://example.invalid/403")
        worker.choice = ProviderChoice("Codex", "test", "high", "session")
        run_start, _, _, _ = worker.prepare_repository()
        (self.repo / "completed.txt").write_text("done\n", encoding="utf-8")
        committed = worker.commit_completed_work(run_start)
        self.assertNotEqual(committed, run_start)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertIn("#403", self.git("log", "-1", "--format=%B"))

    def test_start_comment_is_posted_once_by_selected_provider(self) -> None:
        self.worker.issue = IssueContext(407, "Start notice", "", [], "https://example.invalid/407")
        self.worker.choice = ProviderChoice("Codex", "test-model", "high", "session")
        self.worker.save_new_state(self.worker.issue, self.worker.choice, self.base_sha)
        with (
            mock.patch.object(self.worker, "comments", return_value=[]),
            mock.patch.object(self.worker.github, "gh", return_value="") as github,
        ):
            self.worker.post_started_comment()
            self.worker.post_started_comment()
        github.assert_called_once()
        arguments, provider, body = github.call_args.args
        self.assertEqual(provider, "codex")
        self.assertIn("issue", arguments)
        self.assertIn("**Codex Bot** started working on this issue", body)
        self.assertIn("- Model: `test-model`", body)
        self.assertIn("- Branch: `main`", body)
        self.assertTrue(is_worker_comment({"body": body}))
        self.assertTrue(self.worker.read_state()["started_comment_posted"])

    def test_existing_start_marker_repairs_state_without_duplicate_comment(self) -> None:
        self.worker.issue = IssueContext(408, "Crash-safe notice", "", [], "https://example.invalid/408")
        self.worker.choice = ProviderChoice("Claude", "test-model", "high", "session")
        self.worker.save_new_state(self.worker.issue, self.worker.choice, self.base_sha)
        marker = self.worker.started_comment_marker()
        with (
            mock.patch.object(self.worker, "comments", return_value=[{"body": marker}]),
            mock.patch.object(self.worker.github, "gh", return_value="") as github,
        ):
            self.worker.post_started_comment()
        github.assert_not_called()
        self.assertTrue(self.worker.read_state()["started_comment_posted"])

    def test_dry_run_does_not_post_start_comment(self) -> None:
        args = build_parser().parse_args(
            [
                "--repo-dir", str(self.repo), "--state-dir", str(self.state), "--no-email",
                "--dry-run", "--no-require-bot-auth", "--gh-bin", "/usr/bin/false",
                "--claude-bin", "", "--codex-bin", "",
            ]
        )
        worker = Worker(Config.from_args(args))
        worker.issue = IssueContext(409, "Dry run", "", [], "https://example.invalid/409")
        with (
            mock.patch.object(worker, "claude_capacity", return_value=0),
            mock.patch.object(worker, "codex_capacity", return_value=0),
            mock.patch.object(worker, "post_started_comment") as start_comment,
        ):
            self.assertEqual(worker.run_selected_issue(), 0)
        start_comment.assert_not_called()

    def test_opposite_provider_approves_pull_request(self) -> None:
        worker = self.pr_worker()
        worker.issue = IssueContext(404, "Approval", "", [], "https://example.invalid/404")
        worker.choice = ProviderChoice("Claude", "test", "high", "session")
        with mock.patch.object(worker.github, "gh", return_value="") as github:
            reviewer = worker.approve_pull_request("https://example.invalid/pull/404")
        self.assertEqual(reviewer, "codex")
        self.assertEqual(github.call_args.args[1], "codex")
        self.assertIn("--approve", github.call_args.args[0])

    def test_followup_uses_a_new_comment_specific_branch(self) -> None:
        worker = self.pr_worker()
        worker.issue = IssueContext(
            404,
            "Follow-up",
            "",
            [],
            "https://example.invalid/404",
            work_type="followup",
            trigger_comment_id=9876,
        )
        worker.choice = ProviderChoice("Codex", "test", "high", "session")
        self.assertEqual(worker.expected_branch(), "swarm/codex/issue-404-followup-9876")

    def test_pr_completion_returns_clean_checkout_to_updated_main(self) -> None:
        worker = self.pr_worker()
        worker.issue = IssueContext(405, "Return to main", "", [], "https://example.invalid/405")
        worker.choice = ProviderChoice("Codex", "test", "high", "session")
        run_start, _, _, _ = worker.prepare_repository()
        (self.repo / "merged.txt").write_text("merged\n", encoding="utf-8")
        worker.commit_completed_work(run_start)
        branch = worker.expected_branch()
        self.git("push", "-q", "-u", "origin", branch)

        merger = self.root / "merger"
        subprocess.run(
            ["git", "clone", "-q", "--branch", "main", str(self.remote), str(merger)], check=True
        )
        subprocess.run(["git", "-C", str(merger), "config", "user.name", "merger"], check=True)
        subprocess.run(
            ["git", "-C", str(merger), "config", "user.email", "merger@example.invalid"], check=True
        )
        subprocess.run(["git", "-C", str(merger), "fetch", "-q", "origin", branch], check=True)
        subprocess.run(
            ["git", "-C", str(merger), "merge", "-q", "--no-ff", "FETCH_HEAD", "-m", "merge issue"],
            check=True,
        )
        subprocess.run(["git", "-C", str(merger), "push", "-q", "origin", "main"], check=True)
        merged_main = subprocess.run(
            ["git", "-C", str(merger), "rev-parse", "HEAD"], text=True,
            stdout=subprocess.PIPE, check=True,
        ).stdout.strip()

        synchronized = worker.return_to_synchronized_main(branch)
        self.assertEqual(synchronized, merged_main)
        self.assertEqual(self.git("branch", "--show-current"), "main")
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertNotIn(branch, self.git("branch", "--format=%(refname:short)").splitlines())

    def test_paused_pr_branch_returns_to_main_and_restores_its_own_branch(self) -> None:
        worker = self.pr_worker()
        worker.issue = IssueContext(406, "Paused PR", "", [], "https://example.invalid/406")
        worker.choice = ProviderChoice("Claude", "test", "high", "session-406")
        worker.prepare_repository()
        state = worker.read_state()
        state.update({"status": "quota_paused", "quota_pause_count": 1})
        worker.write_state(state)
        (self.repo / "paused.txt").write_text("paused work\n", encoding="utf-8")
        worker.suspend_paused()
        paused_file = worker.paused_dir / "406.json"
        self.assertEqual(self.git("branch", "--show-current"), "main")
        self.assertTrue(paused_file.is_file())

        worker.restore_paused(paused_file)
        self.assertEqual(self.git("branch", "--show-current"), "swarm/claude/issue-406")
        self.assertEqual((self.repo / "paused.txt").read_text(), "paused work\n")
        self.assertTrue(worker.in_progress_file.is_file())

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
        self.assertEqual(args.delivery_mode, "pull-request")
        self.assertTrue(args.require_bot_auth)
        self.assertTrue(args.auto_approve)
        self.assertTrue(args.auto_merge)
        overridden = build_parser().parse_args(
            ["--github-repository", "example/repo", "--preferred-provider", "codex", "--delivery-mode", "pull-request"]
        )
        self.assertEqual(overridden.github_repository, "example/repo")
        self.assertEqual(overridden.preferred_provider, "codex")


class RunnerTestCase(unittest.TestCase):
    def test_schedule_parser_and_next_run(self) -> None:
        args = runner_module.build_parser().parse_args(
            [
                "--schedule-mode", "custom",
                "--schedule-time", "14:30",
                "--schedule-days", "mon,wed,fri",
                "--no-email",
            ]
        )
        runner = runner_module.Runner(args, [])
        monday_before = dt.datetime(2026, 8, 31, 13, 0).astimezone()
        self.assertEqual(runner.next_scheduled_run(monday_before).weekday(), 0)
        self.assertEqual(runner.next_scheduled_run(monday_before).strftime("%H:%M"), "14:30")
        monday_after = dt.datetime(2026, 8, 31, 15, 0).astimezone()
        self.assertEqual(runner.next_scheduled_run(monday_after).weekday(), 2)

    def test_invalid_schedule_values_are_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            runner_module.build_parser().parse_args(["--schedule-time", "25:00"])
        with self.assertRaises(SystemExit):
            runner_module.build_parser().parse_args(["--schedule-days", "monday,nonesday"])

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
            subprocess.run(["git", "-C", str(repo), "init", "-q", "-b", "main"], check=True)
            subprocess.run(["git", "-C", str(repo), "config", "user.name", "runner test"], check=True)
            subprocess.run(
                ["git", "-C", str(repo), "config", "user.email", "runner@example.invalid"], check=True
            )
            subprocess.run(["git", "-C", str(repo), "commit", "-q", "--allow-empty", "-m", "base"], check=True)
            remote = root / "remote.git"
            subprocess.run(["git", "init", "-q", "--bare", str(remote)], check=True)
            subprocess.run(["git", "-C", str(repo), "remote", "add", "origin", str(remote)], check=True)
            subprocess.run(["git", "-C", str(repo), "push", "-q", "-u", "origin", "main"], check=True)
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

    def test_scheduler_fast_forwards_before_copying_worker_snapshot(self) -> None:
        with tempfile.TemporaryDirectory(prefix="swarm-runner-sync-test.") as temporary:
            root = Path(temporary)
            repo = root / "repo"
            remote = root / "remote.git"
            state = root / "state"
            result_file = root / "result.txt"
            subprocess.run(["git", "init", "-q", "--bare", str(remote)], check=True)
            subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)
            for name, value in (("user.name", "runner test"), ("user.email", "runner@example.invalid")):
                subprocess.run(["git", "-C", str(repo), "config", name, value], check=True)
            worker = repo / "worker.py"
            worker.write_text(
                "import os\nfrom pathlib import Path\n"
                "Path(os.environ['FAKE_RESULT_FILE']).write_text('old')\n",
                encoding="utf-8",
            )
            subprocess.run(["git", "-C", str(repo), "add", "worker.py"], check=True)
            subprocess.run(["git", "-C", str(repo), "commit", "-q", "-m", "old worker"], check=True)
            subprocess.run(["git", "-C", str(repo), "remote", "add", "origin", str(remote)], check=True)
            subprocess.run(["git", "-C", str(repo), "push", "-q", "-u", "origin", "main"], check=True)

            updater = root / "updater"
            subprocess.run(
                ["git", "clone", "-q", "--branch", "main", str(remote), str(updater)], check=True
            )
            for name, value in (("user.name", "updater"), ("user.email", "updater@example.invalid")):
                subprocess.run(["git", "-C", str(updater), "config", name, value], check=True)
            (updater / "worker.py").write_text(
                "import os\nfrom pathlib import Path\n"
                "Path(os.environ['FAKE_RESULT_FILE']).write_text('new')\n",
                encoding="utf-8",
            )
            subprocess.run(["git", "-C", str(updater), "add", "worker.py"], check=True)
            subprocess.run(["git", "-C", str(updater), "commit", "-q", "-m", "new worker"], check=True)
            subprocess.run(["git", "-C", str(updater), "push", "-q", "origin", "main"], check=True)
            remote_sha = subprocess.run(
                ["git", "-C", str(updater), "rev-parse", "HEAD"], text=True,
                stdout=subprocess.PIPE, check=True,
            ).stdout.strip()

            args = runner_module.build_parser().parse_args(
                [
                    "--repo-dir", str(repo), "--state-dir", str(state), "--worker", str(worker),
                    "--once", "--no-email", "--pgrep-bin", "",
                ]
            )
            with mock.patch.dict(os.environ, {"FAKE_RESULT_FILE": str(result_file)}):
                self.assertEqual(runner_module.Runner(args, []).run(), 0)

            self.assertEqual(result_file.read_text(encoding="utf-8"), "new")
            local_sha = subprocess.run(
                ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True,
                stdout=subprocess.PIPE, check=True,
            ).stdout.strip()
            self.assertEqual(local_sha, remote_sha)

    def test_scheduler_does_not_switch_an_active_issue_checkout(self) -> None:
        with tempfile.TemporaryDirectory(prefix="swarm-runner-active-test.") as temporary:
            root = Path(temporary)
            repo = root / "repo"
            state = root / "state"
            subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)
            subprocess.run(["git", "-C", str(repo), "config", "user.name", "runner test"], check=True)
            subprocess.run(
                ["git", "-C", str(repo), "config", "user.email", "runner@example.invalid"], check=True
            )
            subprocess.run(["git", "-C", str(repo), "commit", "-q", "--allow-empty", "-m", "base"], check=True)
            subprocess.run(["git", "-C", str(repo), "switch", "-q", "-c", "swarm/codex/issue-114"], check=True)
            state.mkdir()
            (state / "in-progress-issue.json").write_text("{}\n", encoding="utf-8")
            (repo / "dirty.txt").write_text("active work\n", encoding="utf-8")
            args = runner_module.build_parser().parse_args(
                ["--repo-dir", str(repo), "--state-dir", str(state), "--no-email"]
            )
            runner = runner_module.Runner(args, [])

            self.assertTrue(runner.synchronize_repository())
            branch = subprocess.run(
                ["git", "-C", str(repo), "branch", "--show-current"], text=True,
                stdout=subprocess.PIPE, check=True,
            ).stdout.strip()
            self.assertEqual(branch, "swarm/codex/issue-114")
            self.assertTrue((repo / "dirty.txt").is_file())


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
