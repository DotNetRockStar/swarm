# Automated issue worker

`swarm_issue_worker.sh` handles at most one issue per invocation:

1. Retry any pending GitHub comment or completion email from a successful commit.
2. Choose Claude when both its five-hour and weekly windows have at least 5%
   remaining; otherwise choose Codex when all reported Codex windows do.
3. Fetch open issues assigned to `DotNetRockStar`, then choose the oldest one
   that has not already completed successfully.
4. Run the selected agent in the SWARM repository and require it to leave one
   or more descendant commits on a clean `main` branch.
5. Ensure the commit message references the issue number.
6. Comment on the GitHub issue with the AI tool, model, effort, commit, and the
   concise final AI summary.
7. Add the `Ready For Testing` label to the completed GitHub issue.
8. Send the SMTP notification and record the issue number outside the repo.

The worker uses an atomic PID lock, so a second invocation exits when an earlier
run is still active. State and logs default to
`~/.local/state/swarm-issue-worker`.
An issue is recorded there after its notification succeeds, which prevents an
open issue from being implemented again on the next tick. Failed AI runs are
not recorded and are retried later. The worker never pushes or closes issues.

Preview a read-only selection run:

```bash
SWARM_ISSUE_WORKER_DRY_RUN=1 ./scripts/swarm_issue_worker.sh
```

Set `SWARM_SMTP_CREDENTIALS_FILE` to a settings file containing `EMAIL_FROM`,
`SMTP_HOST`, `SMTP_PORT`, `SMTP_USERNAME`, and the TLS settings, but **not** an
SMTP password. Then run the worker immediately and every ten minutes in the
current terminal:

```bash
export SWARM_SMTP_CREDENTIALS_FILE=/path/to/smtp-settings
./scripts/install_swarm_issue_cron.sh
```

The runner securely prompts for the SMTP password with input hidden. It keeps
the password in memory only, removes it from the worker environment before an
AI tool starts, and sends it to the mail helper over stdin. The foreground
runner hides Codex's implementation transcript and shows only its final summary,
while retaining full failure diagnostics in `last-ai-diagnostic.log`. Visible
output is appended to
`~/.local/state/swarm-issue-worker/cron.log`. Press Ctrl+C to stop it. It also
removes the marked crontab block created by older versions. To remove only that
legacy block without starting the runner, use `--remove`. Override the
ten-minute interval with `SWARM_ISSUE_WORKER_INTERVAL_SECONDS`.

Useful overrides include `SWARM_REPO_DIR`, `SWARM_GITHUB_REPOSITORY`,
`SWARM_GITHUB_ASSIGNEE`, `SWARM_MIN_REMAINING_PERCENT`, `SWARM_CLAUDE_MODEL`,
`SWARM_CODEX_MODEL`, `SWARM_CLAUDE_EFFORT`, `SWARM_CODEX_EFFORT`,
`SWARM_READY_FOR_TESTING_LABEL`, `SWARM_EMAIL_TO`,
`SWARM_SMTP_CREDENTIALS_FILE`, and
`SWARM_ISSUE_WORKER_STATE_DIR`.
