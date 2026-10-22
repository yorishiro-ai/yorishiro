#!/usr/bin/env -S uv run
"""Run the integration suite and verify its selected database/queue topology."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MISMATCH_TOKEN = "YORISHIRO_TOPOLOGY_REJECTED"
TOPOLOGIES = {
    "postgres-postgres",
    "sqlite-sqlite",
    "postgres-valkey",
    "sqlite-valkey",
}


def main() -> int:
    if len(sys.argv) < 2 or sys.argv[1] not in TOPOLOGIES:
        choices = "|".join(sorted(TOPOLOGIES))
        print(f"usage: {Path(sys.argv[0]).name} <{choices}> [cargo test arguments...]", file=sys.stderr)
        return 2
    topology, *cargo_args = sys.argv[1:]
    edition = os.environ.get("YORISHIRO_TEST_EDITION", "ee")
    if edition not in {"ce", "ee"}:
        print("YORISHIRO_TEST_EDITION must be ce or ee", file=sys.stderr)
        return 2
    feature_args = ["--no-default-features", "--features", "community"] if edition == "ce" else ["--features", "enterprise"]
    database, queue = topology.split("-", 1)
    app_url = os.environ.get("DATABASE_URL", "")
    mismatch = (database == "sqlite" and app_url.startswith(("postgres://", "postgresql://"))) or (
        database == "postgres" and app_url.startswith("sqlite:")
    )
    if mismatch:
        print(MISMATCH_TOKEN, file=sys.stderr)
        print(f"ERROR: requested {database} database topology does not match DATABASE_URL", file=sys.stderr)
        return 1
    with tempfile.TemporaryDirectory(prefix="yorishiro-topology-") as temp_dir:
        env = os.environ.copy()
        if database == "sqlite" and queue == "sqlite" and not env.get("QUEUE_URL"):
            queue_path = Path(temp_dir) / "environment-queue.sqlite3"
            env["QUEUE_URL"] = f"sqlite://{queue_path}?mode=rwc"
        marker_dir = Path(temp_dir) / "markers"
        marker_dir.mkdir()
        app_url = env.get("DATABASE_URL", "")
        postgres_url = env.get("YORISHIRO_POSTGRES_TEST_URL")
        if postgres_url:
            env["YORISHIRO_POSTGRES_TEST_URL"] = postgres_url
        elif app_url.startswith(("postgres://", "postgresql://")):
            env["YORISHIRO_POSTGRES_TEST_URL"] = app_url
        else:
            env.pop("YORISHIRO_POSTGRES_TEST_URL", None)
        env.update(
            YORISHIRO_TOPOLOGY_MARKER_DIR=marker_dir,
            YORISHIRO_TEST_TOPOLOGY=topology,
            YORISHIRO_TEST_QUEUE=queue,
        )
        valkey_url = env.get("YORISHIRO_VALKEY_TEST_URL", "")
        queue_url = env.get("QUEUE_URL", "")
        if queue == "valkey":
            if not valkey_url.startswith(("redis://", "rediss://")) and queue_url.startswith(("redis://", "rediss://")):
                valkey_url = queue_url
            if not valkey_url.startswith(("redis://", "rediss://")):
                print("YORISHIRO_VALKEY_TEST_URL must be an explicit Redis URL for Valkey topology", file=sys.stderr)
                return 2
            env["YORISHIRO_VALKEY_TEST_URL"] = valkey_url
            env["QUEUE_URL"] = valkey_url
            env["YORISHIRO_TEST_VALKEY_BASE_URL"] = valkey_url
        else:
            # The CI matrix also sets this variable in SQLite/PostgreSQL cells; do not feed those URLs to Valkey tests.
            env.pop("YORISHIRO_VALKEY_TEST_URL", None)
            env.pop("YORISHIRO_TEST_VALKEY_BASE_URL", None)
        result = subprocess.run(
            ["cargo", "test", "--locked", "--workspace", *feature_args, *cargo_args],
            cwd=ROOT,
            env=env,
            check=False,
        )
        marker = Path(marker_dir, database, queue, "configured-app")
        count = len(list(marker.glob("*"))) if marker.exists() else 0
        print(f"topology {topology}: configured_app={count}")
        if result.returncode:
            return result.returncode
        if not validate_marker_layout(marker_dir, topology):
            print(MISMATCH_TOKEN, file=sys.stderr)
            print("ERROR: unexpected topology marker directories", file=sys.stderr)
            return 1
        if count == 0:
            print(MISMATCH_TOKEN, file=sys.stderr)
            print(f"ERROR: topology {topology} executed zero topology-specific tests", file=sys.stderr)
            return 1
        return 0


def validate_marker_layout(root: Path, topology: str) -> bool:
    root = Path(root)
    database, queue = topology.split("-", 1)
    expected = root / database / queue / "configured-app"
    return expected.is_dir() and not any(
        path.is_dir()
        and not (
            path.relative_to(root).is_relative_to(expected.relative_to(root))
            or expected.relative_to(root).is_relative_to(path.relative_to(root))
        )
        for path in root.rglob("*")
    )


if __name__ == "__main__":
    raise SystemExit(main())
