#!/usr/bin/env -S uv run
"""Self-tests for topology marker validation."""

from pathlib import Path
from tempfile import TemporaryDirectory

from test_topology import validate_marker_layout


def main() -> int:
    with TemporaryDirectory() as directory:
        root = Path(directory)
        expected = root / "sqlite" / "valkey" / "configured-app"
        expected.mkdir(parents=True)
        if not validate_marker_layout(root, "sqlite-valkey"):
            return 1
        (root / "postgres" / "sqlite").mkdir(parents=True)
        if validate_marker_layout(root, "sqlite-valkey"):
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
