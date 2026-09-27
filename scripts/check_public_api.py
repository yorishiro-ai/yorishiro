#!/usr/bin/env python3
"""Check newly added broad Rust public declarations in handwritten source files."""

from __future__ import annotations

import argparse
import os
import pathlib
import re
import subprocess
import sys
from dataclasses import dataclass


ROOT = pathlib.Path(__file__).resolve().parent.parent
ALLOWLIST = ROOT / "scripts" / "public_api_allowlist.tsv"
POLICY_DOC = "docs/en/public-api.md"
WORKTREE = "WORKTREE"
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
KINDS = {"const", "enum", "fn", "mod", "static", "struct", "trait", "type", "use", "field"}
GENERATED_PREFIXES = (
    "src/models/_entities/",
    "src/dtos/",
)


class CheckError(RuntimeError):
    """A public API check could not be completed."""


@dataclass(frozen=True, order=True)
class PublicItem:
    path: str
    kind: str
    symbol: str
    line: int

    def key(self) -> tuple[str, str, str]:
        return self.path, self.kind, self.symbol


@dataclass(frozen=True)
class AllowlistItem:
    path: str
    kind: str
    symbol: str
    reason: str

    def key(self) -> tuple[str, str, str]:
        return self.path, self.kind, self.symbol


@dataclass(frozen=True)
class Token:
    value: str
    line: int


def _is_ident_start(char: str) -> bool:
    return char == "_" or char.isalpha()


def _is_ident_continue(char: str) -> bool:
    return char == "_" or char.isalnum()


def tokens(source: str) -> list[Token]:
    """Tokenize supported declaration forms while skipping comments and literals."""

    result: list[Token] = []
    index = 0
    line = 1
    while index < len(source):
        char = source[index]
        if char.isspace():
            line += char == "\n"
            index += 1
            continue
        if source.startswith("//", index):
            end = source.find("\n", index)
            index = len(source) if end < 0 else end
            continue
        if source.startswith("/*", index):
            depth = 1
            end = index + 2
            while end < len(source) and depth:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            comment = source[index:end]
            line += comment.count("\n")
            index = end
            continue
        if char == '"' or (char == "r" and index + 1 < len(source) and source[index + 1] == '"'):
            index, line = _skip_quoted(source, index, line, raw=False)
            continue
        if char == "r" and re.match(r"r#+\"", source[index:]):
            index, line = _skip_quoted(source, index, line, raw=True)
            continue
        if char == "'" and index + 1 < len(source) and _is_ident_start(source[index + 1]):
            end = index + 2
            while end < len(source) and _is_ident_continue(source[end]):
                end += 1
            if end >= len(source) or source[end] != "'":
                result.append(Token(source[index:end], line))
                index = end
                continue
        if char == "'":
            index, line = _skip_char(source, index, line)
            continue
        if _is_ident_start(char):
            end = index + 1
            while end < len(source) and _is_ident_continue(source[end]):
                end += 1
            result.append(Token(source[index:end], line))
            index = end
            continue
        result.append(Token(char, line))
        index += 1
    return result


def _skip_quoted(source: str, index: int, line: int, raw: bool) -> tuple[int, int]:
    if raw:
        quote = source.find('"', index)
        hashes = quote - index - 1
        end_marker = '"' + ("#" * hashes)
        end = source.find(end_marker, quote + 1)
        end = len(source) if end < 0 else end + len(end_marker)
        return end, line + source[index:end].count("\n")
    index += 1
    while index < len(source):
        if source[index] == "\\":
            index += 2
            continue
        if source[index] == '"':
            return index + 1, line
        line += source[index] == "\n"
        index += 1
    return index, line


def _skip_char(source: str, index: int, line: int) -> tuple[int, int]:
    # A lifetime such as 'a is not a character literal.
    if index + 2 >= len(source) or (source[index + 2] != "'" and source[index + 1] != "\\"):
        return index + 1, line
    end = index + 2
    if source[index + 1] == "\\":
        end += 1
    while end < len(source) and source[end] != "'" and source[end] != "\n":
        end += 1
    return min(end + 1, len(source)), line + source[index : min(end + 1, len(source))].count("\n")


def _restricted_visibility(items: list[Token], index: int) -> int | None:
    next_index = index + 1
    if next_index >= len(items) or items[next_index].value != "(":
        return next_index
    depth = 0
    while next_index < len(items):
        if items[next_index].value == "(":
            depth += 1
        elif items[next_index].value == ")":
            depth -= 1
            if depth == 0:
                return None
        next_index += 1
    return None


def _declaration(items: list[Token], index: int) -> tuple[str, str] | None:
    next_index = _restricted_visibility(items, index)
    if next_index is None or next_index >= len(items):
        return None
    qualifiers: set[str] = set()
    while next_index < len(items) and items[next_index].value in {"async", "const", "unsafe"}:
        qualifiers.add(items[next_index].value)
        next_index += 1
    if next_index < len(items) and items[next_index].value == "fn":
        if next_index + 1 >= len(items):
            return None
        return "fn", items[next_index + 1].value
    if "const" in qualifiers:
        if next_index >= len(items):
            return None
        return "const", items[next_index].value
    if next_index >= len(items):
        return None
    kind = items[next_index].value
    if kind not in KINDS - {"field"}:
        return None
    if kind == "use":
        end = next_index + 1
        while end < len(items) and items[end].value != ";":
            end += 1
        symbol = "".join(item.value for item in items[next_index + 1 : end])
        return kind, symbol or "<use>"
    name_index = next_index + 1
    if kind == "static" and name_index < len(items) and items[name_index].value == "mut":
        name_index += 1
    if name_index >= len(items):
        return None
    return kind, items[name_index].value


@dataclass
class ScopeFrame:
    close: str
    name: str | None = None
    kind: str | None = None
    tuple_struct: bool = False
    tuple_field_index: int = 0
    tuple_field_start: bool = True


def _macro_body_indices(items: list[Token]) -> set[int]:
    """Return macro_rules! declaration/body indices, which are outside this scanner's parser."""

    ignored: set[int] = set()
    index = 0
    while index < len(items):
        if items[index].value != "macro_rules":
            index += 1
            continue
        delimiter = index + 1
        if delimiter >= len(items) or items[delimiter].value != "!":
            index += 1
            continue
        delimiter += 1
        if delimiter < len(items):
            delimiter += 1  # Skip the macro name.
        if delimiter >= len(items) or items[delimiter].value not in {"{", "(", "["}:
            index += 1
            continue
        opening = items[delimiter].value
        matching = {"{": "}", "(": ")", "[": "]"}
        stack = []
        end = delimiter
        while end < len(items):
            value = items[end].value
            if value in matching:
                stack.append(matching[value])
            elif stack and value == stack[-1]:
                stack.pop()
                if not stack:
                    break
            end += 1
        ignored.update(range(index, min(end + 1, len(items))))
        index = end + 1
    return ignored


def _top_level_index(values: list[str], target: str, start: int = 0) -> int | None:
    depths = {"<": 0, "(": 0, "[": 0}
    closing = {">": "<", ")": "(", "]": "["}
    for index in range(start, len(values)):
        value = values[index]
        if value in depths:
            depths[value] += 1
        elif value in closing:
            depths[closing[value]] = max(0, depths[closing[value]] - 1)
        elif value == target and not any(depths.values()):
            return index
    return None


def _normalize_receiver(values: list[str]) -> str:
    """Normalize receiver tokens without collapsing distinct reference or path types."""

    normalized: list[str] = []
    index = 0
    while index < len(values):
        if values[index].startswith("'"):
            index += 1
            continue
        normalized.append(values[index])
        index += 1
    return "".join(normalized)


def _impl_name(items: list[Token], start: int, end: int) -> str:
    """Return the normalized inherent or trait-impl receiver identity."""

    values = [item.value for item in items[start + 1 : end]]
    receiver_start = _top_level_index(values, "for")
    if receiver_start is not None:
        receiver_start += 1
    else:
        receiver_start = 0
        if values and values[0] == "<":
            depth = 0
            for index, value in enumerate(values):
                if value == "<":
                    depth += 1
                elif value == ">":
                    depth -= 1
                    if depth == 0:
                        receiver_start = index + 1
                        break
    receiver_end = _top_level_index(values, "where", receiver_start)
    receiver = _normalize_receiver(values[receiver_start : receiver_end or len(values)])
    return receiver or "<impl>"


def scan_source(path: str, source: str) -> set[PublicItem]:
    """Find broad public items in the supported Rust declaration subset."""

    item_tokens = tokens(source)
    ignored = _macro_body_indices(item_tokens)
    found: dict[tuple[str, str, str], PublicItem] = {}
    delimiters: list[ScopeFrame] = []
    pending_scope: tuple[str, str] | None = None
    impl_start: int | None = None

    def scope_path() -> str:
        return "::".join(frame.name for frame in delimiters if frame.name is not None)

    def qualified(symbol: str) -> str:
        prefix = scope_path()
        return f"{prefix}::{symbol}" if prefix else symbol

    for index, item in enumerate(item_tokens):
        if index in ignored:
            continue

        previous = item_tokens[index - 1].value if index else None
        if item.value == "impl" and previous not in {"->", ":", "=", "return"}:
            impl_start = index
        elif item.value in {"mod", "struct", "enum", "trait"} and index + 1 < len(item_tokens):
            pending_scope = (item.value, item_tokens[index + 1].value)

        if item.value in {"struct", "enum", "trait"} and index + 1 < len(item_tokens):
            pending_scope = (item.value, item_tokens[index + 1].value)
        if item.value == "pub":
            current = delimiters[-1] if delimiters else None
            tuple_field = current and current.tuple_struct and current.tuple_field_start
            next_index = _restricted_visibility(item_tokens, index)
            if tuple_field and next_index is not None:
                public_item = PublicItem(
                    path,
                    "field",
                    qualified(str(current.tuple_field_index)),
                    item.line,
                )
                found[public_item.key()] = public_item
                declaration = None
            else:
                declaration = _declaration(item_tokens, index)
            if declaration:
                kind, symbol = declaration
                public_item = PublicItem(path, kind, qualified(symbol), item.line)
                found[public_item.key()] = public_item
            else:
                if next_index is not None and next_index + 1 < len(item_tokens):
                    field, colon = item_tokens[next_index : next_index + 2]
                    if colon.value == ":" and current and current.kind == "struct" and current.name:
                        public_item = PublicItem(path, "field", qualified(field.value), item.line)
                        found[public_item.key()] = public_item
                    elif tuple_field and not declaration:
                        public_item = PublicItem(
                            path,
                            "field",
                            qualified(str(current.tuple_field_index)),
                            item.line,
                        )
                        found[public_item.key()] = public_item

        if item.value == "," and delimiters and delimiters[-1].tuple_struct:
            delimiters[-1].tuple_field_index += 1
            delimiters[-1].tuple_field_start = True
        elif delimiters and delimiters[-1].tuple_struct and item.value not in {"(", ")"}:
            delimiters[-1].tuple_field_start = False

        if item.value in {"{", "(", "["}:
            close = {"{": "}", "(": ")", "[": "]"}[item.value]
            frame = ScopeFrame(close=close)
            if pending_scope and item.value in {"{", "("}:
                kind, name = pending_scope
                frame.name = name
                frame.kind = kind
                frame.tuple_struct = kind == "struct" and item.value == "("
                pending_scope = None
            elif impl_start is not None and item.value == "{":
                frame.name = _impl_name(item_tokens, impl_start, index)
                frame.kind = "impl"
                impl_start = None
            delimiters.append(frame)
        elif item.value in {"}", ")", "]"} and delimiters:
            delimiters.pop()
        elif item.value == ";":
            pending_scope = None
    return set(found.values())


def is_generated(path: str) -> bool:
    return path.startswith(GENERATED_PREFIXES)


def _git(root: pathlib.Path, *args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=root, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    if result.returncode:
        raise CheckError(result.stderr.strip() or f"git {' '.join(args)} failed")
    return result.stdout


def resolve_ref(root: pathlib.Path, ref: str, label: str) -> str:
    if ref == "0" * 40:
        if label == "base":
            return EMPTY_TREE
        raise CheckError(
            f"unable to resolve public API {label} ref {ref!r}: the all-zero ref is only valid for the base. "
            f"Fetch the ref or set PUBLIC_API_{label.upper()}_REF to a valid commit."
        )
    try:
        return _git(root, "rev-parse", "--verify", f"{ref}^{{commit}}").strip()
    except CheckError as error:
        raise CheckError(
            f"unable to resolve public API {label} ref {ref!r}: {error}. "
            f"Fetch the ref or set PUBLIC_API_{label.upper()}_REF to a valid commit."
        ) from error


def _changed_paths(root: pathlib.Path, base: str, head: str) -> dict[str, str | None]:
    range_args = [base] if head == WORKTREE else [base, head]
    output = _git(
        root,
        "diff",
        "--no-ext-diff",
        "--find-renames",
        "--name-status",
        "-z",
        *range_args,
        "--",
        "src",
        "ee",
    )
    paths: dict[str, str | None] = {}
    parts = [part for part in output.split("\0") if part]
    index = 0
    while index < len(parts):
        status = parts[index]
        index += 1
        if status.startswith(("R", "C")) and index + 1 < len(parts):
            old_path, new_path = parts[index : index + 2]
            index += 2
            if new_path.endswith(".rs"):
                paths[new_path] = old_path
        elif index < len(parts):
            path = parts[index]
            index += 1
            if path.endswith(".rs"):
                paths[path] = path
    if head == WORKTREE:
        untracked = _git(root, "ls-files", "--others", "--exclude-standard", "-z", "--", "src", "ee")
        for path in untracked.split("\0"):
            if path.endswith(".rs"):
                paths[path] = None
    return {path: base_path for path, base_path in paths.items() if not is_generated(path)}


def _source(root: pathlib.Path, head: str, path: str) -> str | None:
    if head == WORKTREE:
        file_path = root / path
        return file_path.read_text() if file_path.exists() else None
    try:
        return _git(root, "show", f"{head}:{path}")
    except CheckError:
        return None


def load_allowlist(path: pathlib.Path) -> dict[tuple[str, str, str], AllowlistItem]:
    result: dict[tuple[str, str, str], AllowlistItem] = {}
    if not path.exists():
        raise CheckError(f"public API allowlist is missing: {path}")
    for line_number, raw_line in enumerate(path.read_text().splitlines(), 1):
        if not raw_line or raw_line.startswith("#"):
            continue
        columns = raw_line.split("\t", 3)
        if len(columns) != 4 or columns[1] not in KINDS:
            raise CheckError(f"{path}:{line_number}: expected path, kind, symbol, reason TSV columns")
        item = AllowlistItem(*columns)
        if not item.reason.strip() or not (item.path.startswith("src/") or item.path.startswith("ee/")):
            raise CheckError(f"{path}:{line_number}: allowlist item needs a source path and reason")
        if is_generated(item.path):
            raise CheckError(f"{path}:{line_number}: generated paths cannot be allowlisted")
        if item.key() in result:
            raise CheckError(f"{path}:{line_number}: duplicate allowlist item {item.key()!r}")
        result[item.key()] = item
    return result


def check_tree(
    root: pathlib.Path,
    base_ref: str,
    head_ref: str = WORKTREE,
    allowlist_path: pathlib.Path | None = None,
) -> list[PublicItem]:
    """Return newly added broad public items not covered by the explicit allowlist."""

    base = resolve_ref(root, base_ref, "base")
    head = WORKTREE if head_ref == WORKTREE else resolve_ref(root, head_ref, "head")
    allowlist = load_allowlist(allowlist_path or root / "scripts" / "public_api_allowlist.tsv")
    findings: list[PublicItem] = []
    for path, base_path in sorted(_changed_paths(root, base, head).items()):
        head_source = _source(root, head, path)
        if head_source is None:
            continue
        base_source = _source(root, base, base_path) if base_path and not is_generated(base_path) else ""
        base_source = base_source or ""
        # Scan the old contents under the head path so a pure rename preserves identity.
        base_items = {item.key() for item in scan_source(path, base_source)}
        for item in scan_source(path, head_source):
            if item.key() not in base_items and item.key() not in allowlist:
                findings.append(item)
    return sorted(findings)


def failure_message(findings: list[PublicItem], allowlist_path: pathlib.Path) -> str:
    lines = ["public API guardrail failed:", "New broad `pub` declarations were added in handwritten Rust."]
    for item in findings:
        lines.append(
            f"  {item.path}:{item.line}: {item.kind} `{item.symbol}`\n"
            f"    Narrow it to pub(crate), pub(super), or private visibility, or add this exact item to "
            f"{allowlist_path} with a specific reason."
        )
    lines.append(f"Policy guidance: {POLICY_DOC}")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-ref", default=os.environ.get("PUBLIC_API_BASE_REF", "HEAD"))
    parser.add_argument("--head-ref", default=os.environ.get("PUBLIC_API_HEAD_REF", WORKTREE))
    parser.add_argument("--allowlist", type=pathlib.Path, default=ALLOWLIST)
    args = parser.parse_args()
    try:
        findings = check_tree(ROOT, args.base_ref, args.head_ref, args.allowlist)
    except CheckError as error:
        print(f"public API guardrail error: {error}", file=sys.stderr)
        return 2
    if findings:
        print(failure_message(findings, args.allowlist), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
