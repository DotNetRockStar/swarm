# SWARM scripts

The unattended Claude/Codex GitHub issue automation is maintained separately
in [SWARM-Media-Steaming/swarm-automation](https://github.com/SWARM-Media-Steaming/swarm-automation).
This repository no longer carries a second copy of its worker scripts.

Other scripts in this directory cover local server startup, Fire TV deployment,
and logs. All closed-loop testing (the media server backend UAT suite, the
Fire TV `tv_e2e_suite.sh`/`tv_uat_suite.sh` suites, the opt-in
`tv_uat_resilience_suite.sh`, and the `full_uat_suite.sh` orchestrator that
runs all of them) lives under [`tests/`](tests/) — see
[`tests/TV_TESTING.md`](tests/TV_TESTING.md) — start there, including its
TL;DR, before running or changing any of them.
