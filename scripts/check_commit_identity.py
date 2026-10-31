#!/usr/bin/env python3
"""Refuse commits whose author or committer is not a GitHub no-reply address.

Every commit in this public repository shows its author and committer to
anyone. The maintainer publishes with a GitHub no-reply address
(``*@users.noreply.github.com``; GitHub's own web commits use
``noreply@github.com``). Anything else — a personal mailbox, a company domain
or a machine placeholder such as ``agent@host`` — is refused.

Outside contributors whose public address is intentional can be listed, one
address per line, in ``.github/allowed-commit-emails``.

Usage::

    python scripts/check_commit_identity.py <rev-range>          # e.g. A..B
    python scripts/check_commit_identity.py --branches --tags
    python scripts/check_commit_identity.py --pre-push <remote> <url> < refs   # git hook

Install the hook once per clone::

    git config core.hooksPath .githooks

The same check runs in CI on every push (``.github/workflows/commit-identity.yml``).
"""
from __future__ import annotations

import os
import subprocess
import sys
from typing import Iterable, List, Optional, Set

ZERO = "0" * 40


def _allowed() -> Set[str]:
    try:
        top = subprocess.run(["git", "rev-parse", "--show-toplevel"], check=True,
                             capture_output=True, text=True).stdout.strip()
        with open(os.path.join(top, ".github", "allowed-commit-emails"), encoding="utf-8") as f:
            return {l.strip().lower() for l in f if l.strip() and not l.startswith("#")}
    except (OSError, subprocess.CalledProcessError):
        return set()


def problem(email: str, allowed: Set[str] = frozenset()) -> Optional[str]:
    """Return why this address must not be published, or None when it is fine."""
    e = (email or "").strip().lower()
    if e.endswith("@users.noreply.github.com") or e == "noreply@github.com" or e in allowed:
        return None
    return "not a GitHub no-reply address"


def check(rev_args: Iterable[str]) -> List[str]:
    out = subprocess.run(
        ["git", "log", "--format=%H%x00%an%x00%ae%x00%cn%x00%ce", *rev_args],
        check=True, capture_output=True, text=True,
    ).stdout
    allowed = _allowed()
    bad = []
    for line in out.splitlines():
        if not line:
            continue
        sha, an, ae, cn, ce = line.split("\x00")
        for role, name, email in (("author", an, ae), ("committer", cn, ce)):
            why = problem(email, allowed)
            if why:
                bad.append(f"{sha[:10]} {role} {name} <{email}>: {why}")
    return bad


def _pre_push_ranges(lines: Iterable[str]) -> List[List[str]]:
    ranges = []
    for line in lines:
        parts = line.split()
        if len(parts) != 4:
            continue
        _local_ref, local_sha, _remote_ref, remote_sha = parts
        if local_sha == ZERO:
            continue
        ranges.append([local_sha, "--not", "--remotes"] if remote_sha == ZERO else [f"{remote_sha}..{local_sha}"])
    return ranges


def main(argv: List[str]) -> int:
    ranges = _pre_push_ranges(sys.stdin) if argv and argv[0] == "--pre-push" else [argv or ["HEAD", "-n", "1"]]
    bad: List[str] = []
    for r in ranges:
        try:
            bad += check(r)
        except subprocess.CalledProcessError as exc:
            print(f"commit identity check: cannot read {' '.join(r)}: {exc.stderr.strip()}", file=sys.stderr)
            return 1
    if bad:
        print("Refusing: these commits would publish a private or placeholder identity:", file=sys.stderr)
        for b in bad:
            print("  " + b, file=sys.stderr)
        print("Fix: git config user.email <login>@users.noreply.github.com, then rewrite the commits.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
