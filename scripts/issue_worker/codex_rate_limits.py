#!/usr/bin/env python3
"""Read Codex ChatGPT quota through the local Codex app-server."""

from __future__ import annotations

import argparse
import json
import select
import subprocess
import sys
import time
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--codex-bin", required=True)
    parser.add_argument("--timeout", type=float, default=20.0)
    return parser.parse_args()


def send(process: subprocess.Popen[str], message: dict[str, Any]) -> None:
    assert process.stdin is not None
    process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    process.stdin.flush()


def receive_response(
    process: subprocess.Popen[str], request_id: int, deadline: float
) -> dict[str, Any]:
    assert process.stdout is not None
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("timed out waiting for Codex app-server")
        readable, _, _ = select.select([process.stdout], [], [], remaining)
        if not readable:
            raise TimeoutError("timed out waiting for Codex app-server")
        line = process.stdout.readline()
        if not line:
            raise RuntimeError("Codex app-server exited before replying")
        message = json.loads(line)
        if message.get("id") == request_id:
            return message


def main() -> int:
    args = parse_args()
    process = subprocess.Popen(
        [args.codex_bin, "app-server", "--listen", "stdio://"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    deadline = time.monotonic() + args.timeout

    try:
        send(
            process,
            {
                "method": "initialize",
                "id": 1,
                "params": {
                    "clientInfo": {
                        "name": "swarm_issue_worker",
                        "title": "SWARM issue worker",
                        "version": "1.0.0",
                    }
                },
            },
        )
        initialized = receive_response(process, 1, deadline)
        if "error" in initialized:
            raise RuntimeError(f"Codex initialization failed: {initialized['error']}")

        send(process, {"method": "initialized", "params": {}})
        send(process, {"method": "account/rateLimits/read", "id": 2, "params": {}})
        response = receive_response(process, 2, deadline)
        if "error" in response:
            raise RuntimeError(f"Codex rate-limit request failed: {response['error']}")
        rate_limits = response.get("result", {}).get("rateLimits")
        if not isinstance(rate_limits, dict):
            raise RuntimeError("Codex returned no rate-limit object")

        print(json.dumps(rate_limits, separators=(",", ":"), sort_keys=True))
        return 0
    except (OSError, ValueError, RuntimeError, TimeoutError) as error:
        print(f"Could not read Codex rate limits: {error}", file=sys.stderr)
        return 1
    finally:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


if __name__ == "__main__":
    raise SystemExit(main())

