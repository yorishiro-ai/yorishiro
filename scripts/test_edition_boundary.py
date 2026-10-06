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
    def rules_for(self, source: str, path: str = "src/x.rs") -> list[str]:
        return [finding.rule for finding in CHECKER.scan(path, source)]

    def test_clean_source_passes(self) -> None:
        source = '#[cfg(feature = "openapi")]\nuse crate::models::api_keys;\nlet tree = 1;\nsqlx("JOIN t ee ON ee.id = e.id");\n'
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
        self.assertEqual(self.rules_for("/// An edition adds its own tools.\nfn f() {}\n"), [])

    def test_aliases_of_the_composition_root_are_found(self) -> None:
        for source in (
            "use crate::edition as overlay;\nfn f() { overlay::ee::run(); }\n",
            "use crate::edition as anything_at_all;\n",
            "use crate::{edition as x, models};\n",
            "use yorishiro::edition::worker_tags;\n",
            "fn f() { crate::edition::workers(); }\n",
        ):
            self.assertIn("the edition composition root as an identifier", self.rules_for(source), source)

    def test_spacing_and_line_breaks_in_paths_are_found(self) -> None:
        for source in (
            "use crate::edition :: ee::app;\n",
            "use crate :: ee :: app;\n",
            "use crate::edition::\nee::app;\n",
            "use crate\n    ::edition\n    ::\n    ee\n    ::app;\n",
            "use crate::{\n    edition::ee as e,\n};\n",
            "use super::ee\n    ;\n",
        ):
            self.assertTrue(self.rules_for(source), source)

    def test_ee_is_found_even_where_the_crate_root_may_name_edition(self) -> None:
        self.assertEqual(self.rules_for("pub mod edition;\npub use edition::{App, worker_tags};\n", "src/lib.rs"), [])
        for source in ("pub use edition::ee as x;\n", "pub use edition::ee;\n", "pub use edition :: ee;\n", "pub use edition::\nee;\n", "use edition as e;\npub use e::ee;\n"):
            self.assertTrue(self.rules_for(source, "src/lib.rs"), source)

    def test_a_variable_named_ee_is_rejected_because_it_cannot_be_told_from_the_module(self) -> None:
        self.assertIn("the ee module as an identifier", self.rules_for("let ee = 1;\n"))

    def test_edition_is_not_allowed_outside_the_crate_root(self) -> None:
        self.assertIn("the edition composition root as an identifier", self.rules_for("pub mod edition;\n", "src/app.rs"))

    def test_strings_comments_and_literals_do_not_trigger_token_rules(self) -> None:
        source = (
            'let sql = "SELECT 1 FROM t ee JOIN edition ON ee.id = edition.id";\n'
            'let raw = r#"edition ee "quoted" edition"#;\n'
            "let bytes = b\"ee\";\n"
            "// edition ee\n"
            "/* edition\n   ee /* nested edition */ still ee */\n"
            "let c = 'e'; let q = '\\''; let s = \"escaped \\\" edition\";\n"
            "fn f<'a>(x: &'a str) -> &'a str { x }\n"
        )
        self.assertEqual(self.rules_for(source), [])

    def test_code_after_a_string_or_comment_is_still_scanned(self) -> None:
        self.assertTrue(self.rules_for('let s = "x"; use crate::edition as e;\n'))
        self.assertTrue(self.rules_for("/* note */ use crate::edition as e;\n"))
        self.assertTrue(self.rules_for("let c = 'x'; let ee = 1;\n"))
        self.assertTrue(self.rules_for('let raw = r#"a"#; use e::ee;\n'))

    def test_findings_keep_their_line_numbers(self) -> None:
        source = "/* two\n   lines */\nuse crate::edition::\nee::x;\n"
        found = CHECKER.scan("src/x.rs", source)
        self.assertEqual(sorted({f.line for f in found}), [3, 4])

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
