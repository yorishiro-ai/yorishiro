#!/usr/bin/env -S uv run
"""Self-tests for topology marker validation."""

from pathlib import Path
from tempfile import TemporaryDirectory
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from subprocess import CompletedProcess
from unittest.mock import patch

from test_topology import MISMATCH_TOKEN, main as run_topology, validate_marker_layout


def run_gate(
    actual_topology: str | None,
    cargo_status: int = 0,
    evidence: bool = True,
    queue_url: str | None = None,
    valkey_url: str | None = None,
    topology: str = "postgres-valkey",
) -> tuple[int, str, str, dict[str, str]]:
    captured_env: dict[str, str] = {}

    def cargo_run(command, *, env, **_kwargs):
        captured_env.update(env)
        if actual_topology is not None:
            database, queue = actual_topology.split("-")
            marker = Path(env["YORISHIRO_TOPOLOGY_MARKER_DIR"], database, queue, "configured-app")
            marker.mkdir(parents=True)
            if evidence:
                (marker / "boot").touch()
        return CompletedProcess(command, cargo_status)

    stdout, stderr = StringIO(), StringIO()
    with (
        patch("sys.argv", ["test_topology.py", topology]),
        patch.dict(
            "os.environ",
            {
                "YORISHIRO_TEST_EDITION": "ee",
                **({"QUEUE_URL": queue_url} if queue_url else {}),
                **({"YORISHIRO_VALKEY_TEST_URL": valkey_url} if valkey_url else {}),
            },
            clear=True,
        ),
        patch("test_topology.subprocess.run", side_effect=cargo_run),
        redirect_stdout(stdout), redirect_stderr(stderr),
    ):
        status = run_topology()
    return status, stdout.getvalue(), stderr.getvalue(), captured_env


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
    status, stdout, stderr, _ = run_gate("sqlite-valkey", valkey_url="redis://127.0.0.1:6379/15")
    assert status != 0
    assert "configured_app=0" in stdout
    assert "unexpected topology marker directories" in stderr
    assert MISMATCH_TOKEN in stderr.splitlines()

    status, _, stderr, _ = run_gate(None, valkey_url="redis://127.0.0.1:6379/15")
    assert status != 0 and MISMATCH_TOKEN in stderr.splitlines()

    status, stdout, stderr, _ = run_gate("postgres-valkey", evidence=False, valkey_url="redis://127.0.0.1:6379/15")
    assert status != 0 and "configured_app=0" in stdout
    assert "zero topology-specific tests" in stderr
    assert MISMATCH_TOKEN in stderr.splitlines()

    status, _, stderr, _ = run_gate("postgres-valkey", valkey_url="redis://127.0.0.1:6379/15")
    assert status == 0 and MISMATCH_TOKEN not in stderr

    # A build/test failure must never satisfy CI's dedicated mismatch-token check.
    status, _, stderr, _ = run_gate("sqlite-valkey", cargo_status=101, valkey_url="redis://127.0.0.1:6379/15")
    assert status == 101 and MISMATCH_TOKEN not in stderr

    # Explicit queue configuration must remain untouched; generated SQLite URLs stay within the runner tempdir.
    status, _, _, env = run_gate(
        "sqlite-sqlite",
        queue_url="sqlite:///caller/isolated-queue.sqlite3?mode=rwc",
        topology="sqlite-sqlite",
    )
    assert status == 0
    assert env["QUEUE_URL"] == "sqlite:///caller/isolated-queue.sqlite3?mode=rwc"

    with patch.dict("os.environ", {}, clear=True):
        status, _, _, env = run_gate("sqlite-sqlite", topology="sqlite-sqlite")
    assert status == 0
    generated_queue_path = Path(env["QUEUE_URL"].removeprefix("sqlite://").split("?", 1)[0])
    assert generated_queue_path.parent.name.startswith("yorishiro-topology-")
    assert not generated_queue_path.parent.is_dir()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
