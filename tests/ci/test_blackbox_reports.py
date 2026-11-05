"""Deliberate report violations must fail the required public-contract gate."""

import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
from check_blackbox_reports import SUITES, load_reports, validate_reports


class ReportGateTests(unittest.TestCase):
    def setUp(self):
        self.expected = {suite: [f"{suite}.one"] for suite in SUITES}
        self.digest = "a" * 64
        self.reports = [
            {
                "suite": suite,
                "edition": "ce",
                "binary_sha256": self.digest,
                "results": [{"id": f"{suite}.one", "status": "passed", "duration_seconds": 0.1}],
            }
            for suite in SUITES
        ]

    def validate(self, reports=None, expected=None):
        validate_reports(self.expected if expected is None else expected, self.reports if reports is None else reports, "ce", self.digest)

    def test_complete_assignment_passes(self):
        self.validate()

    def test_enterprise_requires_only_its_owned_mcp_delta(self):
        report = copy.deepcopy(self.reports[1])
        report["edition"] = "ee"
        tenant = {**report, "suite": "tenant-postgres", "results": [{"id": "tenant.one", "status": "passed", "duration_seconds": 0.1}]}
        validate_reports({"mcp": ["mcp.one"], "tenant-postgres": ["tenant.one"]}, [report, tenant], "ee", self.digest)
        with self.assertRaises(ValueError):
            validate_reports({"mcp": ["mcp.one"], "tenant-postgres": ["tenant.one"]}, [], "ee", self.digest)
        with self.assertRaises(ValueError):
            validate_reports(self.expected, [report], "ee", self.digest)

    def test_single_and_multiple_artifact_layouts_preserve_duplicate_detection(self):
        report = copy.deepcopy(self.reports[1])
        report["edition"] = "ee"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tenant = {**report, "suite": "tenant-postgres", "results": [{"id": "tenant.one", "status": "passed", "duration_seconds": 0.1}]}
            (root / "tenant").mkdir()
            (root / "tenant/results.json").write_text(json.dumps(tenant))
            (root / "results.json").write_text(json.dumps(report))
            validate_reports({"mcp": ["mcp.one"], "tenant-postgres": ["tenant.one"]}, load_reports(root), "ee", self.digest)
            nested = root / "artifact"
            nested.mkdir()
            (nested / "results.json").write_text(json.dumps(report))
            with self.assertRaises(ValueError):
                validate_reports({"mcp": ["mcp.one"], "tenant-postgres": ["tenant.one"]}, load_reports(root), "ee", self.digest)
            (root / "results.json").unlink()
            validate_reports({"mcp": ["mcp.one"], "tenant-postgres": ["tenant.one"]}, load_reports(root), "ee", self.digest)

    def test_missing_duplicate_unknown_and_empty_suites_fail(self):
        for reports in (self.reports[:-1], self.reports + [self.reports[0]], []):
            with self.subTest(reports=reports), self.assertRaises(ValueError):
                self.validate(reports=reports)
        invalid = copy.deepcopy(self.reports)
        invalid[0]["suite"] = "unknown"
        with self.assertRaises(ValueError):
            self.validate(reports=invalid)
        with self.assertRaises(ValueError):
            self.validate(expected={**self.expected, "rest": []})

    def test_missing_duplicate_unknown_and_failed_scenarios_fail(self):
        for results in (
            [],
            self.reports[0]["results"] * 2,
            [{"id": "unknown", "status": "passed", "duration_seconds": 0}],
            [{"id": "rest.one", "status": "failed", "duration_seconds": 0}],
            [{"id": "rest.one", "status": "skipped", "duration_seconds": 0}],
        ):
            invalid = copy.deepcopy(self.reports)
            invalid[0]["results"] = results
            with self.subTest(results=results), self.assertRaises(ValueError):
                self.validate(reports=invalid)

    def test_wrong_binary_edition_and_invalid_timings_fail(self):
        for field, value in (("edition", "ee"), ("binary_sha256", "b" * 64)):
            invalid = copy.deepcopy(self.reports)
            invalid[0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.validate(reports=invalid)
        for duration in (None, -1, float("inf"), float("nan")):
            invalid = copy.deepcopy(self.reports)
            invalid[0]["results"][0]["duration_seconds"] = duration
            with self.subTest(duration=duration), self.assertRaises(ValueError):
                self.validate(reports=invalid)


if __name__ == "__main__":
    unittest.main()
