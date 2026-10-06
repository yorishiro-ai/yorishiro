#!/usr/bin/env python3
"""Fail when `src/` refers to the enterprise overlay or to the composition root.

`src/` is the community base.
It must not name the enterprise feature, the `ee` module, or what the overlay adds, because the only place that knows which editions exist is `edition/`.

Two kinds of check run over every Rust file under `src/`:

* Text rules read the raw lines, comments and strings included: a comment that classifies code by edition is the first step towards a branch that does.
* Token rules read the code with comments and string literals blanked out, so they see through spacing, line breaks and aliases.
  `edition` and `ee` must not appear as identifiers at all, which also rules out `use crate::edition as x;` followed by `x::ee::...`, `crate::edition :: ee`, and a path broken across lines.
  The one exception is `src/lib.rs`, which has to declare the composition root and re-export from it, so it may name `edition` but never `ee`.

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


# `edition` is the composition root's name: only the crate root may declare and re-export it.
EDITION_ALLOWED_IN = frozenset({"src/lib.rs"})

TOKEN_RULES: tuple[tuple[str, re.Pattern[str], bool], ...] = (
    ("the ee module as an identifier", re.compile(r"\bee\b"), False),
    ("the edition composition root as an identifier", re.compile(r"\bedition\b"), True),
)


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    rule: str
    text: str


def strip_code(source: str) -> str:
    """Blanks comments, string literals and char literals, keeping every newline so line numbers survive."""
    out: list[str] = []
    i, n = 0, len(source)

    def blank(text: str) -> str:
        return "".join(c if c == "\n" else " " for c in text)

    while i < n:
        c = source[i]
        two = source[i : i + 2]
        if two == "//":
            j = source.find("\n", i)
            j = n if j < 0 else j
            out.append(blank(source[i:j]))
            i = j
        elif two == "/*":
            depth, j = 1, i + 2
            while j < n and depth:
                if source[j : j + 2] == "/*":
                    depth, j = depth + 1, j + 2
                elif source[j : j + 2] == "*/":
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append(blank(source[i:j]))
            i = j
        elif c in "rb" and (i == 0 or not (source[i - 1].isalnum() or source[i - 1] == "_")) and (
            raw := re.match(r'(?:br|r)(#*)"', source[i:])
        ):
            end = '"' + raw.group(1)
            j = source.find(end, i + raw.end())
            j = n if j < 0 else j + len(end)
            out.append(blank(source[i:j]))
            i = j
        elif c == '"':
            j = i + 1
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            out.append(blank(source[i : j + 1]))
            i = j + 1
        elif c == "'":
            literal = re.match(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^\\'])'", source[i:])
            if literal:
                out.append(blank(literal.group(0)))
                i += literal.end()
            else:
                out.append(c)
                i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def scan(path: str, source: str) -> list[Finding]:
    findings = []
    lines = source.splitlines()
    for number, line in enumerate(lines, 1):
        for rule, pattern in RULES:
            if pattern.search(line):
                findings.append(Finding(path, number, rule, line.strip()))
    flagged = {finding.line for finding in findings}
    for number, line in enumerate(strip_code(source).splitlines(), 1):
        if number in flagged:
            continue
        for rule, pattern, root_name in TOKEN_RULES:
            if root_name and path in EDITION_ALLOWED_IN:
                continue
            if pattern.search(line):
                findings.append(Finding(path, number, rule, lines[number - 1].strip()))
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
