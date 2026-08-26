#!/usr/bin/env python3
"""Register and install the private SWARM Claude/Codex GitHub Apps.

This runs a loopback-only setup page. GitHub requires the owner to approve each
app registration and installation in a signed-in browser; everything after
those approvals (manifest exchange, PEM storage, and config writing) is
automatic.
"""

from __future__ import annotations

import argparse
import html
import json
import os
import secrets
import sys
import threading
import urllib.error
import urllib.parse
import urllib.request
import webbrowser
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

from github_app_auth import DEFAULT_CONFIG_PATH, GitHubAppAuth


PROVIDERS = {
    "claude": {"name": "Swarm Claude Bot", "description": "Claude automation for the SWARM repository."},
    "codex": {"name": "Swarm Codex Bot", "description": "Codex automation for the SWARM repository."},
}


def write_private(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
        stream.write(value)
    path.chmod(0o600)


class SetupState:
    def __init__(self, repository: str, config_path: Path, port: int) -> None:
        self.repository = repository
        self.config_path = config_path
        self.port = port
        self.csrf = {provider: secrets.token_urlsafe(32) for provider in PROVIDERS}
        self.config: dict[str, Any] = {}
        if config_path.exists():
            self.config = json.loads(config_path.read_text(encoding="utf-8"))
        self.complete = threading.Event()
        if all(int(self.config.get(item, {}).get("installation_id", 0)) > 0 for item in PROVIDERS):
            self.complete.set()

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def manifest(self, provider: str) -> dict[str, Any]:
        definition = PROVIDERS[provider]
        return {
            "name": definition["name"],
            "url": f"https://github.com/{self.repository}",
            "description": definition["description"],
            "redirect_url": f"{self.base_url}/callback?provider={provider}",
            "setup_url": f"{self.base_url}/installed?provider={provider}",
            "setup_on_update": True,
            "public": False,
            "request_oauth_on_install": False,
            "default_events": [],
            "default_permissions": {
                "contents": "write",
                "issues": "write",
                "pull_requests": "write",
                "workflows": "write",
            },
        }

    def save_app(self, provider: str, response: dict[str, Any]) -> None:
        slug = str(response["slug"])
        key_path = self.config_path.parent / f"{slug}.pem"
        write_private(key_path, str(response["pem"]))
        self.config[provider] = {
            "app_id": int(response["id"]),
            "installation_id": 0,
            "private_key_path": str(key_path),
            "bot_login": f"{slug}[bot]",
            "bot_name": PROVIDERS[provider]["name"],
        }
        self.save_config()

    def save_installation(self, provider: str, installation_id: int) -> None:
        self.config[provider]["installation_id"] = installation_id
        self.save_config()
        if all(int(self.config.get(item, {}).get("installation_id", 0)) > 0 for item in PROVIDERS):
            self.complete.set()

    def save_config(self) -> None:
        write_private(self.config_path, json.dumps(self.config, indent=2, sort_keys=True) + "\n")


class Handler(BaseHTTPRequestHandler):
    server: "SetupServer"

    def log_message(self, format: str, *args: object) -> None:
        print(f"GitHub App setup: {format % args}", file=sys.stderr)

    def send_html(self, content: str, status: HTTPStatus = HTTPStatus.OK) -> None:
        payload = (
            "<!doctype html><meta charset='utf-8'><title>SWARM GitHub Bots</title>"
            "<style>body{font:16px system-ui;max-width:760px;margin:60px auto;padding:0 24px}"
            "button,a.button{display:inline-block;padding:12px 18px;background:#24292f;color:white;"
            "border:0;border-radius:6px;text-decoration:none;font-weight:600}li{margin:18px 0}code{background:#eee;padding:2px 5px}</style>"
            f"{content}"
        ).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self) -> None:
        parsed = urllib.parse.urlparse(self.path)
        query = urllib.parse.parse_qs(parsed.query)
        if parsed.path == "/":
            items = []
            for provider, definition in PROVIDERS.items():
                configured = int(self.server.state.config.get(provider, {}).get("installation_id", 0)) > 0
                status = "Configured" if configured else "Not configured"
                manifest = html.escape(json.dumps(self.server.state.manifest(provider)))
                state = urllib.parse.quote(self.server.state.csrf[provider])
                items.append(
                    f"<li><strong>{html.escape(definition['name'])}</strong> — {status}<br>"
                    f"<form action='https://github.com/settings/apps/new?state={state}' method='post'>"
                    f"<input type='hidden' name='manifest' value='{manifest}'>"
                    f"<button type='submit'>Create {html.escape(definition['name'])}</button></form></li>"
                )
            self.send_html(
                "<h1>Set up SWARM GitHub bots</h1><p>Create and install both private apps. "
                "On each installation screen choose <strong>Only select repositories</strong> and select "
                f"<code>{html.escape(self.server.state.repository)}</code>.</p><ol>{''.join(items)}</ol>"
            )
            return
        if parsed.path == "/callback":
            provider = query.get("provider", [""])[0]
            code = query.get("code", [""])[0]
            state = query.get("state", [""])[0]
            if provider not in PROVIDERS or state != self.server.state.csrf.get(provider) or not code:
                self.send_html("<h1>Invalid callback</h1>", HTTPStatus.BAD_REQUEST)
                return
            try:
                request = urllib.request.Request(
                    f"https://api.github.com/app-manifests/{urllib.parse.quote(code)}/conversions",
                    data=b"{}",
                    method="POST",
                    headers={
                        "Accept": "application/vnd.github+json",
                        "Content-Type": "application/json",
                        "X-GitHub-Api-Version": "2022-11-28",
                        "User-Agent": "swarm-github-bot-setup",
                    },
                )
                with urllib.request.urlopen(request, timeout=30) as response:
                    app = json.load(response)
                self.server.state.save_app(provider, app)
            except (OSError, KeyError, ValueError, urllib.error.HTTPError) as error:
                self.send_html(f"<h1>App creation failed</h1><pre>{html.escape(str(error))}</pre>", HTTPStatus.BAD_GATEWAY)
                return
            slug = str(app["slug"])
            self.send_html(
                f"<h1>{html.escape(PROVIDERS[provider]['name'])} created</h1>"
                "<p>The private key was stored locally with mode 0600. Now install the app only on the SWARM repository.</p>"
                f"<a class='button' href='https://github.com/apps/{html.escape(slug)}/installations/new'>Install app</a>"
            )
            return
        if parsed.path == "/installed":
            provider = query.get("provider", [""])[0]
            installation = query.get("installation_id", [""])[0]
            if provider not in PROVIDERS or not installation.isdigit():
                self.send_html("<h1>Invalid installation callback</h1>", HTTPStatus.BAD_REQUEST)
                return
            self.server.state.save_installation(provider, int(installation))
            next_provider = next(
                (item for item in PROVIDERS if not self.server.state.config.get(item, {}).get("installation_id")), None
            )
            if next_provider:
                self.send_html(
                    f"<h1>{html.escape(PROVIDERS[provider]['name'])} installed</h1>"
                    f"<p>Return to the setup page to create {html.escape(PROVIDERS[next_provider]['name'])}.</p>"
                    "<a class='button' href='/'>Continue</a>"
                )
            else:
                self.send_html("<h1>Both SWARM bots are configured</h1><p>You may close this tab.</p>")
            return
        self.send_html("<h1>Not found</h1>", HTTPStatus.NOT_FOUND)


class SetupServer(ThreadingHTTPServer):
    def __init__(self, address: tuple[str, int], state: SetupState) -> None:
        super().__init__(address, Handler)
        self.state = state


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default=os.getenv("SWARM_GITHUB_REPOSITORY", "DotNetRockStar/swarm"))
    parser.add_argument("--config", type=Path, default=Path(os.getenv("SWARM_GITHUB_APPS_CONFIG", DEFAULT_CONFIG_PATH)))
    parser.add_argument("--port", type=int, default=0, help="Loopback port; 0 chooses an unused port")
    parser.add_argument("--no-open", action="store_true", help="Print the setup URL without opening a browser")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    config_path = args.config.expanduser().resolve()
    placeholder = SetupState(args.repository, config_path, args.port)
    if placeholder.complete.is_set():
        auth = GitHubAppAuth(config_path)
        for provider in PROVIDERS:
            auth.token(provider)
            print(f"Authenticated {PROVIDERS[provider]['name']} successfully.")
        print(f"Configuration already complete at {config_path}")
        return 0
    server = SetupServer(("127.0.0.1", args.port), placeholder)
    placeholder.port = int(server.server_address[1])
    print(f"SWARM GitHub Bot setup: {placeholder.base_url}/")
    if not args.no_open:
        webbrowser.open(f"{placeholder.base_url}/")
    try:
        while not placeholder.complete.is_set():
            server.handle_request()
    except KeyboardInterrupt:
        return 130
    finally:
        server.server_close()
    auth = GitHubAppAuth(config_path)
    for provider in PROVIDERS:
        auth.token(provider)
        print(f"Authenticated {PROVIDERS[provider]['name']} successfully.")
    print(f"Configuration saved to {config_path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1)
