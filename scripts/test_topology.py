#!/usr/bin/env -S uv run
"""Run the integration suite and verify its selected database/queue topology."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOPOLOGIES = {
    "postgres-postgres",
    "sqlite-sqlite",
    "postgres-valkey",
    "sqlite-valkey",
    "postgres-sqlite",
    "sqlite-postgres",
}


def main() -> int:
    if len(sys.argv) < 2 or sys.argv[1] not in TOPOLOGIES:
        choices = "|".join(sorted(TOPOLOGIES))
        print(f"usage: {Path(sys.argv[0]).name} <{choices}> [cargo test arguments...]", file=sys.stderr)
        return 2
    topology, *cargo_args = sys.argv[1:]
    database, queue = topology.split("-", 1)
    with tempfile.TemporaryDirectory(prefix="yorishiro-topology-markers-") as marker_dir:
        env = os.environ.copy()
        env.update(
            YORISHIRO_TOPOLOGY_MARKER_DIR=marker_dir,
            YORISHIRO_TEST_TOPOLOGY=topology,
            YORISHIRO_TEST_QUEUE=queue,
        )
        result = subprocess.run(
            ["cargo", "test", "--locked", "--workspace", *cargo_args],
            cwd=ROOT,
            env=env,
            check=False,
        )
        marker = Path(marker_dir, database, queue)
        count = len(list(marker.glob("*"))) if marker.exists() else 0
        print(f"topology {topology}: executed={count}")
        if result.returncode:
            return result.returncode
        if count == 0:
            print(f"ERROR: topology {topology} executed zero topology-specific tests", file=sys.stderr)
            return 1
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
