#!/usr/bin/env python3
"""Fail when `src/` refers to the enterprise overlay.

`src/` is the community base.
It must not name the enterprise feature, the `ee` module, or what the overlay adds, because the only place that knows which editions exist is `edition/`.
This is a text check over every Rust file under `src/`, comments included: a comment that classifies code by edition is the first step towards a branch that does.

Usage: check_edition_boundary.py [repository-root]
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

RULES: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("the enterprise cargo feature", re.compile(r"""feature\s*=\s*["']enterprise["']""")),
    ("the ee module path", re.compile(r"\b(?:crate|super|self)::(?:edition::)?ee\b|\bedition::ee\b")),
    ("a path into ee/", re.compile(r"(?<![\w.])ee/")),
    ("the word enterprise", re.compile(r"enterprise", re.IGNORECASE)),
    ("the infer-fill worker", re.compile(r"infer[-_]fill", re.IGNORECASE)),
)


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    rule: str
    text: str


def scan(path: str, source: str) -> list[Finding]:
    findings = []
    for number, line in enumerate(source.splitlines(), 1):
        for rule, pattern in RULES:
            if pattern.search(line):
                findings.append(Finding(path, number, rule, line.strip()))
    return findings


def check_tree(root: Path) -> list[Finding]:
    findings = []
    for file in sorted((root / "src").rglob("*.rs")):
        findings.extend(scan(file.relative_to(root).as_posix(), file.read_text()))
    return findings


def main(argv: list[str]) -> int:
    root = Path(argv[1]).resolve() if len(argv) > 1 else ROOT
    findings = check_tree(root)
    if not findings:
        return 0
    print("edition boundary check failed: src/ must not refer to the enterprise overlay.", file=sys.stderr)
    for finding in findings:
        print(f"  {finding.path}:{finding.line}: {finding.rule}: {finding.text}", file=sys.stderr)
    print(
        "Edition-specific wiring belongs in edition/, and the overlay's own code in ee/. See .agents/rules/editions.md.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
