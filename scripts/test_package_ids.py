#!/usr/bin/env python3
"""Verify current edition package IDs used by the transition harness."""

from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "packaging/test-transitions.sh"


def current_package_id(source: str) -> str:
    function = SCRIPT.read_text().split("current_package_id() {\n", 1)[1].split("\n}", 1)[0]
    result = subprocess.run(
        ["bash", "-c", f'current_package_id() {{ {function}\n}}; current_package_id "$1"', "test", source],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def main() -> int:
    source = SCRIPT.read_text()
    expected = {"ce": "yorishiro-ce", "ee": "yorishiro-ee"}
    for edition, package in expected.items():
        assert current_package_id(edition) == package
    function = source.split("current_package_id() {\n", 1)[1].split("\n}", 1)[0]
    invalid = subprocess.run(
        ["bash", "-c", f'current_package_id() {{ {function}\n}}; current_package_id bad'],
        capture_output=True,
        text=True,
    )
    assert invalid.returncode == 2
    assert "dpkg-query -W -f='\\\\${Status}' ${from_package}" in source
    assert 'rpm -q ${from_package}' in source
    assert 'test "\\$old_query_status" -eq 1' in source
    for transition in (
        "run_deb_transition ce ee",
        "run_deb_transition ee ce",
        "run_rpm_transition ce ee",
        "run_rpm_transition ee ce",
    ):
        assert transition in source
    assert "nfpm package" not in source
    print("current-edition package IDs and transitions: passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
