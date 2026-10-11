#!/usr/bin/env -S uv run
"""Check the deliberately explicit CI test matrix without a YAML dependency."""

from collections import Counter
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent.parent
TOPOLOGIES = {("postgres", "postgres"), ("sqlite", "sqlite"), ("postgres", "valkey"), ("sqlite", "valkey")}


def validate(workflow: str) -> None:
    # Accept only our simple include-only scalar format; fail closed if an axis or YAML structure is added.
    job = workflow.split("\n  test:\n", 1)[1].split("\n    services:\n", 1)[0]
    assert "${{ matrix.edition }}" in job.split("\n", 1)[0], "job names must identify the edition"
    matrix = job.split("\n      matrix:\n", 1)[1]
    lines = [line for line in matrix.splitlines() if line.strip() and not line.lstrip().startswith("#")]
    assert lines.pop(0) == "        include:", "test matrix must be include-only"
    entries = []
    for line in lines:
        match = re.fullmatch(r"          - edition: (ce|ee)", line)
        if match:
            entries.append({"edition": match[1]})
            continue
        field = re.fullmatch(r"            ([a-z_]+): (.+)", line)
        assert field and entries, f"unexpected matrix structure: {line}"
        assert field[1] not in entries[-1], f"duplicate matrix field: {field[1]}"
        entries[-1][field[1]] = field[2]
    required = {"edition", "db", "backend", "queue", "queue_key", "loco_env", "database_url", "queue_url"}
    for entry in entries:
        assert set(entry) == required, f"unexpected fields: {entry}"
        db, queue = entry["backend"], entry["queue_key"]
        assert (db, queue) in TOPOLOGIES, f"forbidden topology: {db}-{queue}"
        assert entry["db"] == {"postgres": "Postgres", "sqlite": "SQLite"}[db]
        assert entry["queue"] == {"postgres": "Postgres", "sqlite": "SQLite", "valkey": "Valkey"}[queue]
        assert entry["loco_env"] == f"test_{db}" + (f"_{queue}" if db != queue else "")
        assert entry["database_url"].startswith(f"{db}:")
        assert entry["queue_url"].startswith(f"{'redis' if queue == 'valkey' else queue}:")
    actual = Counter((entry["edition"], entry["backend"], entry["queue_key"]) for entry in entries)
    expected = Counter((edition, db, queue) for edition in ("ce", "ee") for db, queue in TOPOLOGIES)
    assert actual == expected, f"expected exactly one entry per edition/topology: {actual}"


def main() -> int:
    validate((ROOT / ".github/workflows/ci.yml").read_text())
    print("CI matrix: four permitted topologies x two editions = eight unique jobs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
