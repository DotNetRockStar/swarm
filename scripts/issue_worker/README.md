# SWARM automation scripts

## Automated issue worker

The issue worker is implemented in Python:

- `swarm_issue_worker.py` performs one issue attempt and exits.
- `install_swarm_issue_cron.py` is the foreground scheduler that repeatedly
  invokes the worker. The historical name is retained even though it no longer
  installs cron.
- The matching `.sh` files are compatibility launchers. Existing commands can
  keep using them; all behavior lives in Python.
- `github_app_auth.py` generates short-lived GitHub App installation tokens and
  supplies provider-specific Git commit identities.
- `setup_github_bots.py` performs the one-time Claude Bot and Codex Bot setup.

The Python implementation reads all existing shell-worker state files, so an
active or quota-paused attempt survives the migration.

### TL;DR: set up and run

From the repository root, create and install both GitHub Apps once:

```bash
./scripts/issue_worker/setup.sh
./scripts/issue_worker/github_app_auth.py check --provider claude
./scripts/issue_worker/github_app_auth.py check --provider codex
```

The app setup defaults to `DotNetRockStar/swarm` and writes the required app
IDs, installation IDs, bot identities, and PEM paths to
`~/.config/swarm/github-apps.json`. No app parameter is required afterward
when that default location and repository are used.

The bot-authored branch/PR/approval/merge workflow is now the default. Run:

```bash
./scripts/issue_worker/install_swarm_issue_cron.py \
  --smtp-credentials-file /path/to/smtp-settings
```

The only required runtime choice is notification handling: pass
`--smtp-credentials-file PATH` to enable email, or replace that option with
`--no-email`. The scheduler otherwise uses the defaults listed below. It runs
in the terminal, processes issues back to back, waits 600 seconds while idle,
and stops cleanly with Ctrl+C. Add `--once` for one scheduler iteration, or
`--dry-run --no-email --once` to preview issue selection without making
changes.

Prerequisites are `python3`, `git`, `gh`, `claude`, and `codex` on
`PATH`; the Claude and Codex CLIs must already be authenticated. Use
`worker.env.example` or the parameter tables below only when overriding the
built-in defaults.

### What happens during a run

1. Retry any pending GitHub completion comment, label, or email.
2. Check saved quota-paused sessions. The worker checks each issue's current
   GitHub state before checking quota or restoring work. A closed issue is
   archived as `closed_while_paused` without a comment or AI run, and the
   worker continues to the next eligible issue. Otherwise, a recovered
   provider resumes its exact model and session; unavailable sessions remain
   shelved and do not block other issues.
3. Fetch open issues assigned to the configured assignee.
4. Reconcile trusted completion markers whose commits are already on local
   `main`.
5. Build one queue of ready initial and follow-up work and select the lowest
   numeric issue number. For example, issue #50 is always chosen before #55.
6. For a follow-up, prefer the provider that did not perform the previous pass.
   For a new issue, prefer Claude by default. Both choices honor quota checks.
7. Fast-forward local `main`, save recovery ownership, and create a clean
   provider branch from that exact commit.
8. Post one concise provider-bot comment with the model and branch so observers
   can see that work started, then run Claude or Codex non-interactively. The
   worker commits any completed changes the AI leaves uncommitted and ensures
   the issue is referenced.
9. Push the branch, create a PR against `main`, approve it with the opposite
   bot, merge it, delete the branch, and return the clean local checkout to
   synchronized `main`.
10. Post the structured completion summary, add `Ready For Testing`, send the
    email, and update local state.

The prompt explicitly tells both agents that the invocation is unattended:
they must not ask questions or wait for confirmation. They should make safe,
maintainable decisions and report a blocker only when required credentials,
authority, or external information are genuinely unavailable.

### Default behavior

Defaults remain the same as the former shell worker:

- Repository: `DotNetRockStar/swarm`
- Local repository: parent of the `scripts` directory
- Assignee and trusted follow-up author: `DotNetRockStar`
- Preferred first provider: Claude
- Minimum remaining quota: 10 percent in every active window
- Claude: `claude-sonnet-5`, effort `high`
- Codex: `gpt-5.6-sol`, effort `high`
- Completion label: `Ready For Testing`
- Poll interval: 600 seconds
- Cargo cleanup threshold: 5 GiB
- Bot authentication: required
- Delivery: provider branch, PR against `main`, opposite-bot approval, and
  automatic merge
- State: `~/.local/state/swarm-issue-worker`

Run a read-only selection preview:

```bash
./scripts/issue_worker/swarm_issue_worker.py --dry-run
```

Start the foreground scheduler:

```bash
./scripts/issue_worker/install_swarm_issue_cron.py \
  --smtp-credentials-file /path/to/smtp-settings
```

The scheduler securely prompts for the SMTP password and keeps it only in
memory. Use `--no-email` for an installation that intentionally does not send
notifications. Press Ctrl+C to stop.

Before each immutable worker snapshot is created, the scheduler returns an
idle, clean checkout to the configured base branch and performs a
fast-forward-only pull from the configured remote. Worker changes merged by a
prior run are therefore available to the next run automatically. A saved
in-progress attempt keeps ownership of its branch and worktree until recovery
finishes; dirty work without saved ownership, diverged history, or a failed
pull safely defers AI work instead of overwriting anything.

### Parameters

Command-line options override environment variables; environment variables
override built-in defaults. Run either script with `--help` for the complete
current list. [`worker.env.example`](worker.env.example) shows every commonly
used setting with its current default. [`github-apps.example.json`](github-apps.example.json)
documents the generated app-credential schema without containing usable credentials.

| Worker option | Environment variable | Default |
| --- | --- | --- |
| `--repo-dir` | `SWARM_REPO_DIR` | repository containing `scripts` |
| `--state-dir` | `SWARM_ISSUE_WORKER_STATE_DIR` | `~/.local/state/swarm-issue-worker` |
| `--github-repository` | `SWARM_GITHUB_REPOSITORY` | `DotNetRockStar/swarm` |
| `--assignee` | `SWARM_GITHUB_ASSIGNEE` | `DotNetRockStar` |
| `--trusted-followup-author` | `SWARM_TRUSTED_FOLLOWUP_AUTHORS` | `DotNetRockStar` |
| `--completion-author` | `SWARM_COMPLETION_AUTHORS` | `DotNetRockStar` |
| `--ready-label` | `SWARM_READY_FOR_TESTING_LABEL` | `Ready For Testing` |
| `--minimum-remaining-percent` | `SWARM_MIN_REMAINING_PERCENT` | `10` |
| `--preferred-provider` | `SWARM_PREFERRED_PROVIDER` | `claude` |
| `--claude-model` | `SWARM_CLAUDE_MODEL` | `claude-sonnet-5` |
| `--codex-model` | `SWARM_CODEX_MODEL` | `gpt-5.6-sol` |
| `--claude-effort` | `SWARM_CLAUDE_EFFORT` | `high` |
| `--codex-effort` | `SWARM_CODEX_EFFORT` | `high` |
| `--email-to` | `SWARM_EMAIL_TO` | `mr_jerrodh@hotmail.com` |
| `--smtp-credentials-file` | `SWARM_SMTP_CREDENTIALS_FILE` | unset |
| `--github-apps-config` | `SWARM_GITHUB_APPS_CONFIG` | `~/.config/swarm/github-apps.json` |
| `--[no-]require-bot-auth` | `SWARM_REQUIRE_BOT_AUTH` | enabled |
| `--delivery-mode` | `SWARM_DELIVERY_MODE` | `pull-request` |
| `--[no-]auto-approve` | `SWARM_AUTO_APPROVE` | enabled |
| `--[no-]auto-merge` | `SWARM_AUTO_MERGE` | enabled |
| `--branch-prefix` | `SWARM_BRANCH_PREFIX` | `swarm` |
| `--base-branch` | `SWARM_BASE_BRANCH` | `main` |
| `--remote-name` | `SWARM_GIT_REMOTE` | `origin` |
| `--merge-method` | `SWARM_PR_MERGE_METHOD` | `merge` |
| `--github-host` | `SWARM_GITHUB_HOST` | `github.com` |

`--trusted-followup-author` and `--completion-author` may be repeated. Their
environment equivalents accept comma-separated names. Worker-generated
comments are always excluded from follow-up instructions even when their bot
account is trusted for completion markers.

| Scheduler option | Environment variable | Default |
| --- | --- | --- |
| `--interval-seconds` | `SWARM_ISSUE_WORKER_INTERVAL_SECONDS` | `600` |
| `--cargo-target-max-gib` | `SWARM_CARGO_TARGET_MAX_GIB` | `5` |
| `--worker` | `SWARM_ISSUE_WORKER_PATH` | `swarm_issue_worker.py` |
| `--repo-dir` | `SWARM_REPO_DIR` | repository containing `scripts` |
| `--state-dir` | `SWARM_ISSUE_WORKER_STATE_DIR` | worker state directory |
| `--log-path` | `SWARM_ISSUE_WORKER_LOG_PATH` | `STATE_DIR/cron.log` |
| `--cargo-target-dir` | `SWARM_CARGO_TARGET_DIR` | `REPO_DIR/target` |
| `--transcode-pattern` | `SWARM_TRANSCODE_PROCESS_PATTERN` | SWARM HLS FFmpeg command pattern |
| `--git-bin` | `GIT_BIN` | `git` from `PATH` |
| `--base-branch` | `SWARM_BASE_BRANCH` | `main` |
| `--remote-name` | `SWARM_GIT_REMOTE` | `origin` |

The scheduler accepts worker-only options and forwards unknown arguments to the
worker. Shared repository and state options are forwarded through the worker's
environment. `--once` performs a single scheduler iteration, and
`--check-transcode-active` exits 0 only when a SWARM HLS FFmpeg process is
active.

### GitHub Claude Bot and Codex Bot

Use two private GitHub Apps instead of personal access tokens. An installation
token identifies the app itself, so issues, PRs, comments, labels, pushes, and
merges use the matching bot. Local Git commits are a separate identity layer;
the worker sets both Git author and committer name/email for the selected app.

Run the setup assistant:

```bash
./scripts/issue_worker/setup.sh
```

`setup.sh` starts the loopback-only UI, opens the default browser to its setup
page, and remains attached to the terminal. Press Ctrl+C in that terminal to
stop it. Arguments are forwarded to the Python setup program, for example:

```bash
./scripts/issue_worker/setup.sh --repository DotNetRockStar/swarm
```

GitHub requires one signed-in approval for each app registration and one for
each installation. On the installation screen choose **Only select
repositories** and select `swarm`. The setup assistant then:

- creates private `Swarm Claude Bot` and `Swarm Codex Bot` apps;
- requests Contents, Issues, Pull requests, and Workflows write access;
- requests no webhook URL or events;
- saves each PEM key with mode `0600`;
- saves app and installation IDs to `~/.config/swarm/github-apps.json`;
- verifies that both apps can generate installation tokens.

No personal token is copied into this configuration. Installation tokens live
only in worker memory and are refreshed before expiry. The worker refuses PEM
files readable by group or other users.

After both apps are installed, verify them:

```bash
./scripts/issue_worker/github_app_auth.py check --provider claude
./scripts/issue_worker/github_app_auth.py check --provider codex
```

Any local automation can run a GitHub CLI command as a specific bot without
handling tokens itself. For example:

```bash
./scripts/issue_worker/github_app_auth.py exec --provider codex -- \
  gh issue create --repo DotNetRockStar/swarm --title "Example" --body "Created by Codex"

./scripts/issue_worker/github_app_auth.py exec --provider claude -- \
  gh pr create --repo DotNetRockStar/swarm --fill
```

The same wrapper exports the matching Git author and committer identity to
commands that create local commits. When the wrapped command is `git`, it also
uses an ephemeral `GIT_ASKPASS` helper so HTTPS pushes authenticate as the bot
without placing the installation token in argv, a remote URL, or Git config.

Then enforce bot attribution:

```bash
./scripts/issue_worker/install_swarm_issue_cron.py \
  --require-bot-auth \
  --smtp-credentials-file /path/to/smtp-settings
```

Use `--no-require-bot-auth` only when intentionally allowing an unconfigured
provider to fall back to the existing `gh` login.

### Pull-request delivery

Every issue now uses a provider-specific branch, PR, approval, and merge by
default:

```bash
./scripts/issue_worker/install_swarm_issue_cron.py \
  --smtp-credentials-file /path/to/smtp-settings
```

Initial branches are named `swarm/claude/issue-N` or
`swarm/codex/issue-N`; follow-up passes add `-followup-COMMENT_ID` so a prior
merged PR cannot be mistaken for the new round. Before branching, the worker
fast-forwards local `main` and writes recovery state. The agent never pushes
directly; after it finishes, the worker commits any remaining changes with that
provider's identity, obtains a short-lived app token, pushes without persisting
credentials, and creates the PR. The opposite bot approves the PR, the
implementing bot merges it, the remote and local issue branches are deleted,
and the checkout returns to clean, synchronized `main`.

The legacy `--delivery-mode local-main` remains available only as an explicit
override and must be paired with `--no-auto-approve --no-auto-merge`.

### State and recovery

Only one worker invocation and one foreground scheduler may own a state
directory. PID-directory locks recover automatically after a crash.

Important state files:

- `in-progress-issue.json` — active provider, model, session, exact synchronized
  base, branch ownership, and recovery commits. It is written before branch
  creation so Ctrl+C cannot create an unowned issue branch. It also records
  whether the start notice was posted; the hidden notice marker prevents a
  crash between posting and saving state from creating duplicate comments.
- `quota-paused-issues/N.json` — shelved sessions that must resume with their
  original provider.
- `closed-paused-issues/N.json` — recovery metadata and any stash reference for
  an issue that was closed while paused. This is an audit/recovery archive, not
  a completion marker. If the issue is reopened, it is eligible for a fresh
  branch and AI session.
- `pending-email.json` — a successful commit whose GitHub or email bookkeeping
  still needs retrying.
- `completed-issues` — issue numbers completed locally.
- `last-ai-output.log` — final agent response.
- `last-ai-diagnostic.log` — detailed Codex events or failure diagnostics.
- `cron.log` — foreground scheduler output.

Exit code 10 means an issue completed and the scheduler should check again
immediately. Exit code 11 means a quota-paused session was safely shelved and a
different issue may run. Exit code 0 means nothing can be worked now. Other
codes are retried after the configured interval.

Before each attempt, the scheduler defers while a SWARM HLS transcode is active.
Afterward, it runs `cargo clean` when `target` exceeds the configured threshold
and no Cargo/Rust compiler process is active. Rust builds retain the repository's
compact debug and non-incremental settings.

### Testing

```bash
./scripts/issue_worker/test_swarm_issue_worker.py
python3 -m py_compile scripts/issue_worker/*.py
```

The regression suite exercises bot/human comment trust, opposite-provider
follow-ups, quota shelving/restoration, damaged recovery-SHA repair, concurrent
dirty worktree preservation, Markdown rendering, parameter defaults, transcode
detection, and the lifetime runner lock.
