#!/usr/bin/env -S uv run
"""Run the Redis-compatible queue checks."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from urllib.parse import urlparse


ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    url = os.environ.get("YORISHIRO_REDIS_TEST_URL")
    if not url:
        print("YORISHIRO_REDIS_TEST_URL is required", file=sys.stderr)
        return 2
    if not url.startswith(("redis://", "rediss://")):
        print("YORISHIRO_REDIS_TEST_URL must use redis:// or rediss://", file=sys.stderr)
        return 2
    if urlparse(url).path != "/15":
        print("YORISHIRO_REDIS_TEST_URL must select the reserved test database 15", file=sys.stderr)
        return 2

    env = os.environ.copy()
    env.setdefault("LOCO_ENV", "test_sqlite")
    commands = [
        [
            "cargo",
            "test",
            "--locked",
            "--lib",
            "workers::dispatch::redis_routing::a_class_drains_its_own_jobs_behind_a_backlog_of_another_class",
            "--",
            "--exact",
            "--nocapture",
        ],
        [
            "cargo",
            "test",
            "--locked",
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
            "--test",
            "mod",
            "config::load::redis_test_config_keeps_queue_independent_from_database",
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
