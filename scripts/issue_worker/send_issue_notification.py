#!/usr/bin/env python3
"""Send an issue-worker email using settings plus a password on stdin."""

from __future__ import annotations

import argparse
import re
import shlex
import smtplib
import ssl
import sys
from email.message import EmailMessage
from email.utils import formataddr
from pathlib import Path


EMAIL_KEYS = {
    "EMAIL_FROM",
    "EMAIL_FROM_DISPLAY_NAME",
    "SMTP_HOST",
    "SMTP_PORT",
    "SMTP_USERNAME",
    "SMTP_USE_SSL",
    "SMTP_STARTTLS",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--credentials", type=Path, required=True)
    parser.add_argument("--password-stdin", action="store_true", required=True)
    parser.add_argument("--to", required=True)
    parser.add_argument("--issue-number", required=True)
    parser.add_argument("--issue-title", required=True)
    parser.add_argument("--issue-url", required=True)
    parser.add_argument("--ai", required=True)
    parser.add_argument(
        "--notification-type",
        choices=("completed", "quota-paused"),
        default="completed",
    )
    parser.add_argument("--model", default="")
    parser.add_argument("--session-id", default="")
    parser.add_argument("--commit-sha", default="")
    parser.add_argument("--commit-message", default="")
    return parser.parse_args()


def decode_value(raw_value: str) -> str:
    try:
        values = shlex.split(raw_value, comments=False, posix=True)
    except ValueError as error:
        raise ValueError(f"invalid quoted SMTP credential value: {error}") from error
    return " ".join(values)


def load_email_credentials(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    in_email_section = False

    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            if in_email_section and values and not line:
                break
            continue
        if line.lower() == "email":
            in_email_section = True
            continue
        if not in_email_section:
            continue
        if "=" not in line:
            if values:
                break
            continue

        key, raw_value = line.removeprefix("export ").split("=", 1)
        key = key.strip()
        if key in EMAIL_KEYS:
            values[key] = decode_value(raw_value.strip())

    required = {"EMAIL_FROM", "SMTP_HOST", "SMTP_PORT"}
    missing = sorted(required - values.keys())
    if missing:
        raise ValueError(f"missing SMTP settings in email section: {', '.join(missing)}")
    return values


def is_true(value: str | None) -> bool:
    return (value or "").strip().lower() in {"1", "true", "yes", "on"}


def safe_header(value: str) -> str:
    return re.sub(r"[\r\n]+", " ", value).strip()


def main() -> int:
    args = parse_args()
    try:
        settings = load_email_credentials(args.credentials)
        password = sys.stdin.readline().rstrip("\r\n")
        if not password:
            raise ValueError("an SMTP password is required on stdin")
        port = int(settings["SMTP_PORT"])
        use_ssl = is_true(settings.get("SMTP_USE_SSL"))
        use_starttls = is_true(settings.get("SMTP_STARTTLS"))

        message = EmailMessage()
        sender = safe_header(settings["EMAIL_FROM"])
        display_name = safe_header(settings.get("EMAIL_FROM_DISPLAY_NAME", ""))
        message["From"] = formataddr((display_name, sender)) if display_name else sender
        message["To"] = safe_header(args.to)
        if args.notification_type == "quota-paused":
            message["Subject"] = (
                f"SWARM issue #{safe_header(args.issue_number)} paused: "
                f"{safe_header(args.ai)} usage exhausted"
            )
            message.set_content(
                "\n".join(
                    [
                        f"Issue: #{args.issue_number} {args.issue_title}",
                        f"URL: {args.issue_url}",
                        f"AI: {args.ai}",
                        f"Model: {args.model}",
                        f"Session: {args.session_id}",
                        "Status: Paused because the selected AI no longer has sufficient usage.",
                        "The worker saved this session and will resume it automatically when that same AI has usage available again.",
                    ]
                )
            )
        else:
            message["Subject"] = (
                f"SWARM issue #{safe_header(args.issue_number)} worked by "
                f"{safe_header(args.ai)}"
            )
            message.set_content(
                "\n".join(
                    [
                        f"Issue: #{args.issue_number} {args.issue_title}",
                        f"URL: {args.issue_url}",
                        f"AI: {args.ai}",
                        f"Commit: {args.commit_sha}",
                        f"Commit message: {args.commit_message}",
                    ]
                )
            )

        context = ssl.create_default_context()
        if use_ssl:
            smtp_connection = smtplib.SMTP_SSL(
                settings["SMTP_HOST"], port, timeout=30, context=context
            )
        else:
            smtp_connection = smtplib.SMTP(settings["SMTP_HOST"], port, timeout=30)

        with smtp_connection as smtp:
            if not use_ssl:
                smtp.ehlo()
                if use_starttls:
                    smtp.starttls(context=context)
                    smtp.ehlo()
            username = settings.get("SMTP_USERNAME", "")
            if not username:
                raise ValueError("SMTP_USERNAME is required when using a password")
            smtp.login(username, password)
            smtp.send_message(message)
        return 0
    except (OSError, ValueError, smtplib.SMTPException) as error:
        print(f"SMTP notification failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
