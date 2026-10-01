#!/usr/bin/env -S uv run
"""Run the Redis/Valkey queue checks."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    url = os.environ.get("YORISHIRO_REDIS_TEST_URL")
    if not url:
        print("YORISHIRO_REDIS_TEST_URL is required", file=sys.stderr)
        return 2
    if not url.startswith(("redis://", "rediss://")):
        print("YORISHIRO_REDIS_TEST_URL must use redis:// or rediss://", file=sys.stderr)
        return 2

    env = os.environ.copy()
    env.setdefault("LOCO_ENV", "test_sqlite")
    commands = [
        [
            "cargo",
            "test",
            "--locked",
            "--features",
            "test-support",
            "--test",
            "mod",
            "requests::queue::redis_bounded_scan_is_observable_at_the_queue_boundary",
            "--",
            "--exact",
            "--nocapture",
        ],
        [
            "cargo",
            "test",
            "--locked",
            "--features",
            "test-support",
            "--test",
            "mod",
            "config::load::valkey_test_config_keeps_queue_external_to_postgres",
            "--",
            "--exact",
            "--nocapture",
        ],
    ]
    for command in commands:
        subprocess.run(command, cwd=ROOT, env=env, check=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
