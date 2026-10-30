#!/usr/bin/env python3
"""Keep Rust runtime environment reads in the application and test registries.

This is a practical token check, not Rust name resolution.
Qualified reads and imports of reader functions are rejected, including aliases.
Process operations, compile-time macros and the generic test guard remain separate.
"""

import re
import sys
from pathlib import Path

from check_edition_boundary import strip_code

ROOT = Path(__file__).resolve().parent.parent
ALLOWED = {"src/data/env.rs", "tests/env.rs", "tests/mod.rs"}
READERS = r"(?:var|var_os|vars|vars_os)"
QUALIFIED = re.compile(rf"::\s*{READERS}\b(?!\s*!)")
IMPORT = re.compile(rf"\buse\b[^;]*\b{READERS}\b[^;]*;", re.DOTALL)
GLOB = re.compile(r"\buse\s+(?:::)?std\s*::\s*env\s*::\s*\*\s*;")


def violations(path: str, source: str) -> list[int]:
    if path in ALLOWED:
        return []
    code = strip_code(source)
    return sorted({code.count("\n", 0, match.start()) + 1
                   for pattern in (QUALIFIED, IMPORT, GLOB)
                   for match in pattern.finditer(code)})


def main() -> int:
    failed = False
    for directory in ("src", "ee", "tests", "migration/src"):
        for file in sorted((ROOT / directory).rglob("*.rs")):
            path = file.relative_to(ROOT).as_posix()
            for line in violations(path, file.read_text()):
                print(f"{path}:{line}: centralize runtime environment reads in data/env.rs or tests/env.rs")
                failed = True
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
