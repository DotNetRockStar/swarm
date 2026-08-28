# SWARM scripts

The unattended Claude/Codex GitHub issue automation now lives in
[`issue_worker/`](issue_worker/README.md). Existing top-level shell commands
remain as compatibility launchers:

```bash
./scripts/swarm_issue_worker.sh --dry-run
./scripts/install_swarm_issue_cron.sh
./scripts/test_swarm_issue_worker.sh
```

To create or verify the Claude and Codex GitHub Apps through the local setup UI:

```bash
./scripts/issue_worker/setup.sh
```

Other scripts in this directory cover local server startup, Fire TV deployment,
and logs. All closed-loop testing (the media server backend UAT suite, the
Fire TV `tv_e2e_suite.sh`/`tv_uat_suite.sh` suites, the opt-in
`tv_uat_resilience_suite.sh`, and the `full_uat_suite.sh` orchestrator that
runs all of them) lives under [`tests/`](tests/) — see
[`tests/TV_TESTING.md`](tests/TV_TESTING.md) — start there, including its
TL;DR, before running or changing any of them.
