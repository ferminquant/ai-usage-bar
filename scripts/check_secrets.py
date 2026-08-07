#!/usr/bin/env python3
"""Fail on high-confidence credentials in tracked repository files.

This is intentionally a narrow scanner, not a claim that regexes prove a
repository contains no secrets. It reports only the file, line, and rule name;
never print the matching content. Deliberate redaction tests use short values
that cannot satisfy these high-confidence production-key patterns.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path


RULES: tuple[tuple[str, re.Pattern[str]], ...] = (
    (
        "private-key",
        re.compile(r"-----BEGIN (?:[A-Z0-9]+ )?PRIVATE KEY-----"),
    ),
    (
        "github-token",
        re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b"),
    ),
    (
        "provider-key-prefix",
        re.compile(r"\b(?:sk-(?:proj-)?|oc_sk_)[A-Za-z0-9_-]{20,}\b"),
    ),
    (
        "jwt",
        re.compile(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b"),
    ),
    (
        "bearer-value",
        re.compile(
            r"(?i)\bBearer\s+(?!<|REDACTED\b)[A-Za-z0-9._~+/=-]{20,}"
        ),
    ),
    (
        "authorization-value",
        re.compile(
            r"(?i)\b(?:Authorization|Proxy-Authorization)\s*[:=]\s*"
            r"(?!<|REDACTED\b)(?:[\"'][A-Za-z0-9._~+/=-]{20,}[\"']|[A-Za-z0-9_~+/=-]{20,})"
        ),
    ),
    (
        "named-secret-value",
        re.compile(
            r"(?i)\b(?:access_token|refresh_token|id_token|api[_-]?key|"
            r"client_secret|password|secret|cookie)\s*[:=]\s*"
            r"(?:[\"'][A-Za-z0-9._~+/=-]{16,}[\"']|[A-Za-z0-9_~+/=-]{16,})"
        ),
    ),
)


def tracked_files(root: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
    )
    return [root / name for name in result.stdout.decode().split("\0") if name]


def scan(root: Path) -> list[tuple[Path, int, str]]:
    findings: list[tuple[Path, int, str]] = []
    for path in tracked_files(root):
        try:
            raw = path.read_bytes()
        except OSError:
            continue
        if b"\0" in raw:
            continue
        text = raw.decode("utf-8", errors="replace")
        for line_number, line in enumerate(text.splitlines(), start=1):
            for name, rule in RULES:
                if rule.search(line):
                    findings.append((path.relative_to(root), line_number, name))
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    root = args.root.resolve()
    findings = scan(root)
    if findings:
        print("High-confidence secret patterns found:", file=sys.stderr)
        for path, line_number, rule in findings:
            print(f"  {path}:{line_number}: {rule}", file=sys.stderr)
        return 1
    print("Secret scan passed: no high-confidence patterns in tracked files.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
