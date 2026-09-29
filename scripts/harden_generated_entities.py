#!/usr/bin/env python3
"""Remove generated Debug leaks from entities that contain secret columns."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ENTITY_DIR = Path("src/models/_entities")
SECRET_FIELDS = {
    "access_token",
    "api_key",
    "client_secret",
    "id_token",
    "private_key",
    "refresh_token",
    "secret",
    "signing_key",
    "token",
    "webhook_secret",
}


def model_span(source: str) -> tuple[int, int] | None:
    match = re.search(r"pub struct Model\s*\{(?P<body>.*?)\n\}", source, re.DOTALL)
    if match is None:
        return None
    return match.start("body"), match.end("body")


def secret_fields(source: str) -> list[str]:
    span = model_span(source)
    if span is None:
        return []
    body = source[slice(*span)]
    return re.findall(r"^\s*pub\s+(\w+)\s*:", body, re.MULTILINE).copy()


def harden_model(source: str) -> str:
    span = model_span(source)
    if span is None:
        return source

    fields = [field for field in secret_fields(source) if field in SECRET_FIELDS]
    if not fields:
        return source

    struct_start = source.find("pub struct Model")
    derive_start = source.rfind("#[derive(", 0, struct_start)
    derive_end = source.find(")]", derive_start, struct_start)
    if derive_start < 0 or derive_end < 0:
        raise ValueError("Model derive is missing")

    derives = source[derive_start : derive_end + 2]
    derive_items = [item.strip() for item in derives[9:-2].split(",")]
    derive_items = [item for item in derive_items if item != "Debug"]
    replacement = "#[derive(" + ", ".join(derive_items) + ")]"
    source = source[:derive_start] + replacement + source[derive_end + 2 :]

    for field in fields:
        field_match = re.search(rf"^(\s*)pub\s+{re.escape(field)}\s*:", source, re.MULTILINE)
        if field_match is None:
            raise ValueError(f"secret field {field!r} disappeared")
        line_start = field_match.start()
        previous = source[:line_start].splitlines()[-1] if line_start else ""
        if "serde(skip_serializing)" not in previous:
            indent = field_match.group(1)
            source = (
                source[:line_start]
                + f"{indent}#[serde(skip_serializing)]\n"
                + source[line_start:]
            )

    return source


def verify(source: str, path: Path) -> None:
    span = model_span(source)
    if span is None:
        return
    fields = [field for field in secret_fields(source) if field in SECRET_FIELDS]
    if not fields:
        return

    struct_start = source.find("pub struct Model")
    derive_start = source.rfind("#[derive(", 0, struct_start)
    derive_end = source.find(")]", derive_start, struct_start)
    derive = source[derive_start : derive_end + 2]
    if re.search(r"(?:^|,\s*)Debug(?:,|\))", derive):
        raise ValueError(f"{path}: secret-bearing Model still derives Debug")
    body = source[slice(*model_span(source))]
    for field in fields:
        field_match = re.search(rf"^\s*pub\s+{re.escape(field)}\s*:", body, re.MULTILINE)
        if field_match is None:
            raise ValueError(f"{path}: secret field {field!r} is missing")
        field_prefix = body[: field_match.start()]
        previous_lines = [line for line in field_prefix.splitlines() if line.strip()]
        if not previous_lines or "serde(skip_serializing)" not in previous_lines[-1]:
            raise ValueError(f"{path}: secret field {field!r} is serializable")


def main() -> int:
    for path in sorted(ENTITY_DIR.glob("*.rs")):
        original = path.read_text()
        hardened = harden_model(original)
        if hardened != original:
            path.write_text(hardened)
        verify(hardened, path)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print(f"entity hardening failed: {error}", file=sys.stderr)
        raise SystemExit(1)
