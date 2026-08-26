# Automated issue worker

`swarm_issue_worker.sh` handles at most one issue per invocation, while the
foreground runner immediately invokes it again after each successful issue:

1. Retry any pending GitHub comment or completion email from a successful commit.
2. Fetch open issues assigned to `DotNetRockStar`. Rework a completed issue when
   it has a new comment after the latest worker completion; otherwise choose the
   oldest issue that has not completed successfully.
3. For new issues, choose Claude when both its session and weekly windows have
   at least 10% remaining, otherwise Codex. For follow-ups, prefer whichever of
   Claude or Codex did not perform the latest completed pass.
4. Run the selected agent in the SWARM repository and require it to leave one
   or more descendant commits on `main`.
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
not recorded as complete and are retried later. The worker never pushes or
closes issues.

Completion comments carry a hidden processed-comment watermark. A later
non-worker GitHub comment makes that open, assigned issue actionable again only
when its author is `DotNetRockStar` (or `SWARM_TRUSTED_FOLLOWUP_AUTHOR` when
overridden); comments from every other account are ignored and never inserted
into the AI prompt. Trusted follow-up work takes priority over a new issue. The
rework prompt includes the original issue title, description, and labels; the
prior AI completion comment; the previous commit SHA, message, and changed-file
summary; and every unprocessed trusted follow-up comment in chronological order.
The agent is also directed to inspect the full previous commit diff before
creating a new issue-referencing refinement commit. Trusted comments posted
while a rework is running are beyond its saved watermark and therefore trigger
another pass instead of being hidden by the new completion comment.
For a follow-up pass, the worker prefers the opposite provider from the latest
worker completion (Codex after Claude, or Claude after Codex) to provide an
independent review and reduce same-model bias. If that provider lacks quota, it
falls back to whichever provider can work. Quota-paused sessions are the
exception: they resume their exact pinned provider and session.

Before starting an agent, the worker records the issue number, current base
commit, selected AI, model, effort, and persistent session ID in
`in-progress-issue.json`. If an agent exits before committing, the next run
resumes that exact Claude or Codex session and its existing work instead of
starting a new conversation, switching providers, rejecting the dirty worktree,
or selecting another issue. If the work is committed from another terminal,
the resumed agent verifies the saved descendant commit and can complete the
comment, label, email, and local bookkeeping without adding duplicate code. A
recovered commit that has already been established is never amended merely to
add the issue number; the GitHub completion comment provides the issue-to-commit
link without rewriting history.

When the selected AI runs out of usage during a turn, the worker marks that
session `quota_paused`, posts one idempotent pause comment on the issue, and
sends one pause email. Its state is moved under `quota-paused-issues/`, and any
uncommitted work is shelved so the foreground runner can immediately select a
different ready issue (using whichever provider has capacity). Paused issue
numbers are excluded from fresh and follow-up selection, so no duplicate
session is created. On later runs, a paused session whose pinned provider has
recovered takes priority; its shelved changes and exact Claude or Codex session
are restored before work continues. Trusted issue comments added since the
session last received context are supplied to the resumed session. Only
comments by `SWARM_TRUSTED_FOLLOWUP_AUTHOR` are inserted into an AI prompt.

Agent runs are non-interactive. The prompt directs Claude and Codex to resolve
ambiguity from the issue and repository, make reasonable safe assumptions, and
implement their recommended maintainable approach without asking the user for
confirmation. Claude runs with `bypassPermissions`; Codex runs with approval,
sandbox, and hook-trust prompts bypassed. This gives both tools broad access to
the local machine and network, so run this worker only for repositories and
assigned issues you trust.

Fresh-provider quota checks run only after an eligible issue is found; pinned
providers for shelved sessions are checked first to decide whether they can be
resumed. Claude usage comes from Claude Code's non-interactive `/usage` command,
allowing the CLI to refresh its own OAuth credentials; the worker does not call
Anthropic's private OAuth usage URL. Codex usage comes from the local
`codex app-server` over stdio using the `account/rateLimits/read` method; the
Codex CLI owns its authenticated network connection, so the worker does not
hard-code a remote Codex endpoint.

Preview a read-only selection run:

```bash
SWARM_ISSUE_WORKER_DRY_RUN=1 ./scripts/swarm_issue_worker.sh
```

Set `SWARM_SMTP_CREDENTIALS_FILE` to a settings file containing `EMAIL_FROM`,
`SMTP_HOST`, `SMTP_PORT`, `SMTP_USERNAME`, and the TLS settings, but **not** an
SMTP password. Then run the worker in the current terminal. Queued issues are
worked back to back; the ten-minute delay applies only when the queue is empty,
every remaining issue is quota-paused, neither AI has at least 10% remaining in
every active quota window, or a run fails:

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
legacy block without starting the runner, use `--remove`. Override the idle and
error retry interval with `SWARM_ISSUE_WORKER_INTERVAL_SECONDS`. Rust development
and test builds use compact line-table debug information with incremental
compilation disabled. After each worker run, the runner also uses `cargo clean`
when the shared `target` directory exceeds 1 GiB and no Cargo or Rust compiler
process is active; override that limit with `SWARM_CARGO_TARGET_MAX_GIB`.
Before each worker run, the runner also checks for an active SWARM HLS FFmpeg
process and defers AI/build work for the normal polling interval so background
verification cannot compete with playback. Use
`./scripts/install_swarm_issue_cron.sh --check-transcode-active` to inspect that
preflight result (`0` means active, `1` means idle).
Only one foreground runner may be active for a state directory; a second
invocation exits without starting another polling loop. Temporary failures of
an AI provider's usage-check command are treated as unknown availability, not
as quota exhaustion, so they do not create pause/resume comments or loops.

Useful overrides include `SWARM_REPO_DIR`, `SWARM_GITHUB_REPOSITORY`,
`SWARM_GITHUB_ASSIGNEE`, `SWARM_TRUSTED_FOLLOWUP_AUTHOR`,
`SWARM_MIN_REMAINING_PERCENT`, `SWARM_CLAUDE_MODEL`,
`SWARM_CODEX_MODEL`, `SWARM_CLAUDE_EFFORT`, `SWARM_CODEX_EFFORT`,
`SWARM_READY_FOR_TESTING_LABEL`, `SWARM_EMAIL_TO`,
`SWARM_SMTP_CREDENTIALS_FILE`, and
`SWARM_ISSUE_WORKER_STATE_DIR`.
