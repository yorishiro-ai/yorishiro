#!/usr/bin/env -S uv run --script
"""Run the test suite and verify that only the selected backend gates ran."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    if len(sys.argv) < 2 or sys.argv[1] not in {"postgres", "sqlite"}:
        print(
            "usage: uv run --script scripts/test_backend.py <postgres|sqlite> [cargo test arguments...]",
            file=sys.stderr,
        )
        return 2

    backend, *cargo_args = sys.argv[1:]
    with tempfile.TemporaryDirectory(prefix="yorishiro-backend-markers-") as marker_dir:
        env = os.environ.copy()
        env["YORISHIRO_BACKEND_MARKER_DIR"] = marker_dir
        result = subprocess.run(
            ["cargo", "test", "--locked", "--workspace", "--features", "test-support", *cargo_args],
            cwd=ROOT,
            env=env,
            check=False,
        )

        verification_failed = False
        for suite in ("postgres", "sqlite"):
            executed_dir = Path(marker_dir, suite, "executed")
            skipped_dir = Path(marker_dir, suite, "skipped")
            executed = len(list(executed_dir.glob("*") if executed_dir.exists() else ()))
            skipped_paths = list(skipped_dir.glob("*") if skipped_dir.exists() else ())
            skipped = len(skipped_paths)
            selected = executed + skipped
            print(f"backend suite {suite} gates: selected={selected} executed={executed} skipped={skipped}")
            if skipped_paths:
                print(f"backend suite {suite} skip reason: {skipped_paths[0].read_text()}")

            if suite == backend and executed == 0:
                print(f"ERROR: {backend} job executed zero {backend}-specific tests", file=sys.stderr)
                verification_failed = True
            if suite != backend and executed != 0:
                print(
                    f"ERROR: {backend} job executed {executed} {suite}-specific tests",
                    file=sys.stderr,
                )
                verification_failed = True

        if result.returncode:
            return result.returncode
        return int(verification_failed)


if __name__ == "__main__":
    raise SystemExit(main())
