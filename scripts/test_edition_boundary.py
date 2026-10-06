#!/usr/bin/env python3
"""Tests for check_edition_boundary.py, including deliberate violations."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "check_edition_boundary", Path(__file__).with_name("check_edition_boundary.py")
)
assert SPEC is not None and SPEC.loader is not None
CHECKER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CHECKER
SPEC.loader.exec_module(CHECKER)


class EditionBoundaryTest(unittest.TestCase):
    def rules_for(self, source: str) -> list[str]:
        return [finding.rule for finding in CHECKER.scan("src/x.rs", source)]

    def test_clean_source_passes(self) -> None:
        source = '#[cfg(feature = "openapi")]\nuse crate::models::api_keys;\nlet ee = 1;\nsqlx("JOIN t ee ON ee.id = e.id");\n'
        self.assertEqual(self.rules_for(source), [])

    def test_enterprise_feature_gates_are_found(self) -> None:
        self.assertEqual(self.rules_for('#[cfg(feature = "enterprise")]\nfn f() {}\n'), ["the enterprise cargo feature", "the word enterprise"])
        self.assertIn("the enterprise cargo feature", self.rules_for('#[cfg(not(feature="enterprise"))]\n'))
        self.assertIn("the enterprise cargo feature", self.rules_for("#[cfg_attr(feature = 'enterprise', derive(Debug))]\n"))

    def test_ee_module_paths_are_found(self) -> None:
        for source in (
            "use crate::ee::app;\n",
            "crate::edition::ee::models::x();\n",
            "use super::ee;\n",
            "let _ = edition::ee::y;\n",
        ):
            self.assertIn("the ee module path", self.rules_for(source), source)

    def test_paths_into_ee_directory_are_found(self) -> None:
        self.assertIn("a path into ee/", self.rules_for('#[path = "../ee/mod.rs"]\n'))
        self.assertIn("a path into ee/", self.rules_for("// see ee/app.rs\n"))

    def test_edition_neutral_words_are_allowed(self) -> None:
        self.assertEqual(self.rules_for("//! The composition root names every edition.\n"), [])
        self.assertEqual(self.rules_for("use crate::edition::App;\n"), [])

    def test_overlay_worker_and_wording_are_found(self) -> None:
        self.assertIn("the infer-fill worker", self.rules_for("const QUEUE: &str = \"infer-fill\";\n"))
        self.assertIn("the word enterprise", self.rules_for("// Enterprise only.\n"))

    def test_check_tree_scans_only_src(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src" / "nested").mkdir(parents=True)
            (root / "ee").mkdir()
            (root / "edition").mkdir()
            (root / "src" / "lib.rs").write_text("pub mod nested;\n")
            (root / "ee" / "mod.rs").write_text("// enterprise\n")
            (root / "edition" / "mod.rs").write_text('#[cfg(feature = "enterprise")]\n')
            self.assertEqual(CHECKER.check_tree(root), [])

            (root / "src" / "nested" / "leak.rs").write_text("use crate::ee::app;\n")
            findings = CHECKER.check_tree(root)
            self.assertEqual([(f.path, f.line) for f in findings], [("src/nested/leak.rs", 1)])

    def test_main_reports_a_deliberate_violation_with_a_failing_status(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src" / "lib.rs").write_text('#[cfg(feature = "enterprise")]\nmod ee;\n')
            self.assertEqual(CHECKER.main(["check", str(root)]), 1)
            (root / "src" / "lib.rs").write_text("mod models;\n")
            self.assertEqual(CHECKER.main(["check", str(root)]), 0)

    def test_the_repository_itself_is_clean(self) -> None:
        self.assertEqual(CHECKER.check_tree(CHECKER.ROOT), [])


if __name__ == "__main__":
    unittest.main()
