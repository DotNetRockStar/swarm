#!/usr/bin/env python3
"""GitHub App authentication for SWARM's Claude and Codex automations.

The module intentionally has no third-party dependencies. It signs the GitHub
App JWT with the system OpenSSL binary, exchanges it for a short-lived
installation token, and never writes that token to disk.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


DEFAULT_CONFIG_PATH = Path.home() / ".config" / "swarm" / "github-apps.json"
API_VERSION = "2022-11-28"


def _b64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


def _request(method: str, url: str, bearer: str, payload: dict[str, Any] | None = None) -> Any:
    body = None if payload is None else json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        method=method,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {bearer}",
            "Content-Type": "application/json",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": "swarm-issue-worker",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"GitHub returned HTTP {error.code}: {detail}") from error


@dataclass(frozen=True)
class AppDefinition:
    app_id: int
    installation_id: int
    private_key_path: Path
    bot_login: str
    bot_name: str
    bot_email: str

    @classmethod
    def from_mapping(cls, provider: str, value: dict[str, Any], base_dir: Path) -> "AppDefinition":
        required = ("app_id", "installation_id", "private_key_path", "bot_login")
        missing = [key for key in required if not value.get(key)]
        if missing:
            raise ValueError(f"{provider} GitHub App config is missing: {', '.join(missing)}")
        key_path = Path(os.path.expanduser(str(value["private_key_path"])))
        if not key_path.is_absolute():
            key_path = base_dir / key_path
        login = str(value["bot_login"])
        return cls(
            app_id=int(value["app_id"]),
            installation_id=int(value["installation_id"]),
            private_key_path=key_path,
            bot_login=login,
            bot_name=str(value.get("bot_name") or login.removesuffix("[bot]").replace("-", " ").title()),
            bot_email=str(value.get("bot_email") or ""),
        )


class GitHubAppAuth:
    """Loads per-provider app definitions and mints in-memory tokens."""

    def __init__(self, config_path: Path | str = DEFAULT_CONFIG_PATH, openssl_bin: str = "openssl") -> None:
        self.config_path = Path(config_path).expanduser()
        self.openssl_bin = openssl_bin
        self._definitions: dict[str, AppDefinition] = {}
        self._tokens: dict[str, tuple[str, float]] = {}
        if self.config_path.exists():
            raw = json.loads(self.config_path.read_text(encoding="utf-8"))
            for provider in ("claude", "codex"):
                if isinstance(raw.get(provider), dict):
                    self._definitions[provider] = AppDefinition.from_mapping(
                        provider, raw[provider], self.config_path.parent
                    )

    def configured(self, provider: str) -> bool:
        return provider.lower() in self._definitions

    def definition(self, provider: str) -> AppDefinition:
        key = provider.lower()
        try:
            definition = self._definitions[key]
        except KeyError as error:
            raise RuntimeError(
                f"No GitHub App is configured for {provider}; expected {self.config_path}"
            ) from error
        if not definition.private_key_path.is_file():
            raise RuntimeError(f"GitHub App private key was not found: {definition.private_key_path}")
        if definition.private_key_path.stat().st_mode & 0o077:
            raise RuntimeError(
                f"GitHub App private key permissions are too broad; run chmod 600 {definition.private_key_path}"
            )
        return definition

    def _jwt(self, definition: AppDefinition) -> str:
        now = int(time.time())
        header = _b64url(json.dumps({"alg": "RS256", "typ": "JWT"}, separators=(",", ":")).encode())
        claims = _b64url(
            json.dumps(
                {"iat": now - 60, "exp": now + 540, "iss": str(definition.app_id)},
                separators=(",", ":"),
            ).encode()
        )
        unsigned = f"{header}.{claims}".encode("ascii")
        signed = subprocess.run(
            [self.openssl_bin, "dgst", "-sha256", "-sign", str(definition.private_key_path)],
            input=unsigned,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if signed.returncode != 0:
            raise RuntimeError(
                f"OpenSSL could not sign the GitHub App JWT: {signed.stderr.decode(errors='replace').strip()}"
            )
        return f"{header}.{claims}.{_b64url(signed.stdout)}"

    def token(self, provider: str) -> str:
        key = provider.lower()
        cached = self._tokens.get(key)
        if cached and cached[1] > time.time() + 120:
            return cached[0]
        definition = self.definition(key)
        response = _request(
            "POST",
            f"https://api.github.com/app/installations/{definition.installation_id}/access_tokens",
            self._jwt(definition),
            {},
        )
        token = str(response["token"])
        # GitHub installation tokens currently last one hour. Cache for at most
        # 50 minutes so clock skew cannot leak an expired token into a long run.
        self._tokens[key] = (token, time.time() + 3000)
        return token

    def bot_environment(self, provider: str) -> dict[str, str]:
        definition = self.definition(provider)
        email = definition.bot_email
        if not email:
            token = self.token(provider)
            profile = _request(
                "GET",
                f"https://api.github.com/users/{urllib.parse.quote(definition.bot_login, safe='')}",
                token,
            )
            email = f"{profile['id']}+{definition.bot_login}@users.noreply.github.com"
        return {
            "GH_TOKEN": self.token(provider),
            "GITHUB_TOKEN": self.token(provider),
            "GIT_AUTHOR_NAME": definition.bot_name,
            "GIT_AUTHOR_EMAIL": email,
            "GIT_COMMITTER_NAME": definition.bot_name,
            "GIT_COMMITTER_EMAIL": email,
        }

    def completion_authors(self) -> set[str]:
        return {definition.bot_login for definition in self._definitions.values()}


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=Path(os.getenv("SWARM_GITHUB_APPS_CONFIG", DEFAULT_CONFIG_PATH)))
    parser.add_argument("--openssl-bin", default=os.getenv("OPENSSL_BIN", "openssl"))
    subparsers = parser.add_subparsers(dest="command", required=True)
    for command in ("identity", "check"):
        sub = subparsers.add_parser(command)
        sub.add_argument("--provider", required=True, choices=("claude", "codex"))
    execute = subparsers.add_parser("exec", help="Run a command as the selected bot")
    execute.add_argument("--provider", required=True, choices=("claude", "codex"))
    execute.add_argument("command_args", nargs=argparse.REMAINDER)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    auth = GitHubAppAuth(args.config, args.openssl_bin)
    if args.command == "identity":
        identity = auth.bot_environment(args.provider)
        identity.pop("GH_TOKEN", None)
        identity.pop("GITHUB_TOKEN", None)
        print(json.dumps(identity, indent=2, sort_keys=True))
    elif args.command == "check":
        definition = auth.definition(args.provider)
        auth.token(args.provider)
        print(f"{args.provider}: authenticated as {definition.bot_login}")
    else:
        command = list(args.command_args)
        if command and command[0] == "--":
            command.pop(0)
        if not command:
            raise ValueError("exec requires a command after --")
        environment = os.environ.copy()
        environment.pop("SWARM_SMTP_PASSWORD", None)
        environment.update(auth.bot_environment(args.provider))
        if Path(command[0]).name == "git":
            # Make HTTPS git operations use the installation token without
            # embedding it in argv, a remote URL, or persistent Git config.
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
                environment.update(
                    {
                        "GIT_ASKPASS": str(askpass),
                        "GIT_TERMINAL_PROMPT": "0",
                        "SWARM_GITHUB_APP_PUSH_TOKEN": environment["GH_TOKEN"],
                    }
                )
                return subprocess.run(command, env=environment, check=False).returncode
        return subprocess.run(command, env=environment, check=False).returncode
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1)
