#!/usr/bin/env -S uv run
"""Self-tests for topology marker validation."""

from pathlib import Path
from tempfile import TemporaryDirectory
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from subprocess import CompletedProcess
from unittest.mock import patch

from test_topology import MISMATCH_TOKEN, main as run_topology, validate_marker_layout


def run_gate(actual_topology: str | None, cargo_status: int = 0, evidence: bool = True) -> tuple[int, str, str]:
    def cargo_run(command, *, env, **_kwargs):
        if actual_topology is not None:
            database, queue = actual_topology.split("-")
            marker = Path(env["YORISHIRO_TOPOLOGY_MARKER_DIR"], database, queue, "configured-app")
            marker.mkdir(parents=True)
            if evidence:
                (marker / "boot").touch()
        return CompletedProcess(command, cargo_status)

    stdout, stderr = StringIO(), StringIO()
    with (
        patch("sys.argv", ["test_topology.py", "postgres-valkey"]),
        patch.dict("os.environ", {"YORISHIRO_TEST_EDITION": "ee"}),
        patch("test_topology.subprocess.run", side_effect=cargo_run),
        redirect_stdout(stdout), redirect_stderr(stderr),
    ):
        status = run_topology()
    return status, stdout.getvalue(), stderr.getvalue()


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
    # Regression: wrong marker directories leave configured_app=0, before the empty-count branch is reached.
    status, stdout, stderr = run_gate("sqlite-valkey")
    assert status != 0
    assert "configured_app=0" in stdout
    assert "unexpected topology marker directories" in stderr
    assert MISMATCH_TOKEN in stderr.splitlines()

    status, _, stderr = run_gate(None)
    assert status != 0 and MISMATCH_TOKEN in stderr.splitlines()

    status, stdout, stderr = run_gate("postgres-valkey", evidence=False)
    assert status != 0 and "configured_app=0" in stdout
    assert "zero topology-specific tests" in stderr
    assert MISMATCH_TOKEN in stderr.splitlines()

    status, _, stderr = run_gate("postgres-valkey")
    assert status == 0 and MISMATCH_TOKEN not in stderr

    # A build/test failure must never satisfy CI's dedicated mismatch-token check.
    status, _, stderr = run_gate("sqlite-valkey", cargo_status=101)
    assert status == 101 and MISMATCH_TOKEN not in stderr
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
