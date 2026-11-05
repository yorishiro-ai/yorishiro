#!/usr/bin/env python3
"""Require every assigned public contract to pass on the built binary."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SUITES = ("rest", "mcp", "cli")


def validate_reports(expected: dict[str, list[str]], reports: list[dict], edition: str, binary_sha256: str) -> None:
    if set(expected) != ({"mcp"} if edition == "ee" else set(SUITES)):
        raise ValueError("expected assignments must include REST, MCP and CLI")
    seen = set()
    for report in reports:
        suite = report.get("suite")
        if suite not in expected or suite in seen:
            raise ValueError(f"unknown or duplicate suite: {suite}")
        seen.add(suite)
        if report.get("edition") != edition or report.get("binary_sha256") != binary_sha256:
            raise ValueError(f"{suite}: report does not match the edition and built binary")
        assignments = expected[suite]
        if not assignments or len(set(assignments)) != len(assignments):
            raise ValueError(f"{suite}: empty or duplicate scenario assignments")
        results = report.get("results", [])
        ids = [result["id"] for result in results]
        if len(set(ids)) != len(ids) or set(ids) != set(assignments):
            raise ValueError(f"{suite}: executed scenarios do not match assignments")
        for result in results:
            duration = result.get("duration_seconds")
            if result.get("status") != "passed":
                raise ValueError(f"{suite}: scenario did not pass: {result['id']}")
            if not isinstance(duration, (int, float)) or not math.isfinite(duration) or duration < 0:
                raise ValueError(f"{suite}: invalid scenario duration")
    if seen != set(expected):
        raise ValueError(f"missing suite reports: {sorted(set(expected) - seen)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reports", required=True, type=Path)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--edition", choices=("ce", "ee"), required=True)
    args = parser.parse_args()
    expected = {}
    for suite in (("mcp",) if args.edition == "ee" else SUITES):
        output = subprocess.check_output(
            [sys.executable, str(ROOT / "tests/blackbox/run.py"), "--suite", suite, "--edition", args.edition, "--list"],
            text=True,
        )
        expected[suite] = json.loads(output)
    reports = [json.loads(path.read_text()) for path in sorted(args.reports.glob("*/results.json"))]
    with args.binary.open("rb") as binary:
        digest = hashlib.file_digest(binary, "sha256").hexdigest()
    validate_reports(expected, reports, args.edition, digest)
    reviewed = json.loads((ROOT / "tests/blackbox/tools.json").read_text())
    inventory = set(reviewed["ce"])
    if args.edition == "ee":
        inventory.update(reviewed["ee_additions"])
    discovered = [report.get("discovered_tools", []) for report in reports if report["suite"] == "mcp"]
    if len(discovered) != 1 or set(discovered[0]) != inventory:
        raise ValueError("MCP discovery does not match the reviewed public-tool inventory")
    calls = [call for report in reports for call in report.get("tool_calls", [])]
    for report in reports:
        for call in report.get("tool_calls", []):
            if (call.get("tool") not in inventory or call.get("scenario") not in expected[report["suite"]]
                    or call.get("outcome") not in ("succeeded", "rejected")):
                raise ValueError("invalid public-tool execution attribution")
    census = {name: {outcome: sorted({call["scenario"] for call in calls
                                    if call["tool"] == name and call["outcome"] == outcome})
                     for outcome in ("succeeded", "rejected")} for name in sorted(inventory)}
    gaps = [name for name, outcomes in census.items() if not outcomes["succeeded"]]
    (args.reports / "tool-coverage.json").write_text(json.dumps({
        "binary_sha256": digest, "edition": args.edition, "tools": census,
        "without_successful_mcp_call": gaps,
        "ee_additions_not_executed": [name for name in reviewed["ee_additions"]
                                     if args.edition == "ce" or name in gaps],
        "complete_public_behavior_coverage": False,
    }, indent=2) + "\n")
    print(f"Public MCP census: {len(inventory) - len(gaps)}/{len(inventory)} tools called successfully; "
          f"{len(gaps)} without successful calls, {len(reviewed['ee_additions'])} EE additions not executed")
    print(f"Public contracts: {len(reports)} suites, {sum(map(len, expected.values()))} assigned scenarios passed on {digest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
