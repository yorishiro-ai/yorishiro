#!/usr/bin/env python3
"""Verify old-package identifiers used by the transition harness."""

from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "packaging/test-transitions.sh"


def old_package_id(source: str) -> str:
    function = SCRIPT.read_text().split("old_package_id() {\n", 1)[1].split("\n}", 1)[0]
    result = subprocess.run(
        ["bash", "-c", f'old_package_id() {{ {function}\n}}; old_package_id "$1"', "test", source],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def main() -> int:
    expected = {"yorishiro": "yorishiro", "ce": "yorishiro-ce", "ee": "yorishiro-ee"}
    for source, package in expected.items():
        assert old_package_id(source) == package
    function = SCRIPT.read_text().split("old_package_id() {\n", 1)[1].split("\n}", 1)[0]
    invalid = subprocess.run(
        ["bash", "-c", f'old_package_id() {{ {function}\n}}; old_package_id bad'],
        capture_output=True,
        text=True,
    )
    assert invalid.returncode == 2
    source = SCRIPT.read_text()
    assert "old_status=\\\"\\$(dpkg-query -W -f='\\${Status}' \\\"\\$OLD_PACKAGE\\\"" in source
    assert "rpm -q \\\"\\$OLD_PACKAGE\\\"" in source
    assert "test \\\"\\$old_query_status\\\" -eq 1" in source
    print("transition old-package IDs: passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
