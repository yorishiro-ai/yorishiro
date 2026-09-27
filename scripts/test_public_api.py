#!/usr/bin/env python3
"""Focused tests for the lightweight public API diff checker."""

from __future__ import annotations

import importlib.util
import pathlib
import subprocess
import sys
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("check_public_api.py")
SPEC = importlib.util.spec_from_file_location("check_public_api", SCRIPT)
CHECKER = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = CHECKER
SPEC.loader.exec_module(CHECKER)


class PublicApiCheckerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tempdir.name)
        self.git("init", "-q")
        self.git("config", "user.email", "test@example.com")
        self.git("config", "user.name", "Public API test")
        self.write("scripts/public_api_allowlist.tsv", "# path\tkind\tsymbol\treason\n")
        self.write("src/internal.rs", "fn unchanged() {}\n")
        self.commit()

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def git(self, *args: str) -> str:
        return subprocess.run(["git", *args], cwd=self.root, check=True, text=True, stdout=subprocess.PIPE).stdout

    def revision(self) -> str:
        return self.git("rev-parse", "HEAD").strip()

    def write(self, path: str, content: str) -> None:
        file_path = self.root / path
        file_path.parent.mkdir(parents=True, exist_ok=True)
        file_path.write_text(content)

    def commit(self) -> None:
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")

    def check(self) -> list:
        return CHECKER.check_tree(self.root, "HEAD", allowlist_path=self.root / "scripts/public_api_allowlist.tsv")

    def test_accidental_added_pub_has_exact_actionable_finding(self) -> None:
        self.write("src/internal.rs", "fn unchanged() {}\npub async fn accidental() {}\n")

        findings = self.check()

        self.assertEqual([(item.path, item.kind, item.symbol, item.line) for item in findings], [("src/internal.rs", "fn", "accidental", 2)])
        message = CHECKER.failure_message(findings, pathlib.Path("scripts/public_api_allowlist.tsv"))
        self.assertIn("src/internal.rs:2: fn `accidental`", message)
        self.assertIn("Narrow it to pub(crate), pub(super), or private", message)

    def test_approved_allowlist_entry_passes(self) -> None:
        self.write("src/internal.rs", "pub struct TransportArgs;\n")
        self.write(
            "scripts/public_api_allowlist.tsv",
            "src/internal.rs\tstruct\tTransportArgs\tREST transport DTO is intentionally public.\n",
        )

        self.assertEqual(self.check(), [])

    def test_unchanged_historical_pub_passes(self) -> None:
        self.write("src/internal.rs", "pub fn historical() {}\n")
        self.commit()
        self.write("src/internal.rs", "pub fn historical() {}\nfn new_private() {}\n")

        self.assertEqual(self.check(), [])

    def test_generated_change_is_ignored(self) -> None:
        self.write("src/models/_entities/generated.rs", "pub struct Generated;\n")
        self.write("src/dtos/generated.rs", "pub struct GeneratedDto;\n")

        self.assertEqual(self.check(), [])

    def test_new_file_is_checked(self) -> None:
        self.write("ee/new_surface.rs", "pub enum NewSurface { Item }\n")

        findings = self.check()

        self.assertEqual([(item.path, item.kind, item.symbol) for item in findings], [("ee/new_surface.rs", "enum", "NewSurface")])

    def test_supported_declaration_forms_are_classified(self) -> None:
        source = """
pub const fn const_fn() {}
pub async fn async_fn() {}
pub unsafe fn unsafe_fn() {}
pub(crate) fn restricted() {}
pub use crate::module::Export;
pub struct Generic<T> { pub field: T, pub(crate) hidden: T }
pub enum Example { Item }
pub trait Behaviour {}
pub type Alias = String;
pub const CONSTANT: usize = 1;
pub static STATIC: usize = 1;
pub static mut MUTABLE: usize = 1;
pub const unsafe fn const_unsafe() {}
pub unsafe const fn unsafe_const() {}
pub mod nested {}
mod outer {
    pub fn same() {}
    mod inner { pub fn same() {} }
}
pub struct Wrapper(pub String, String);
impl Wrapper { pub fn same() {} }
"""

        keys = {(item.kind, item.symbol) for item in CHECKER.scan_source("src/forms.rs", source)}

        self.assertIn(("fn", "const_fn"), keys)
        self.assertIn(("fn", "async_fn"), keys)
        self.assertIn(("fn", "unsafe_fn"), keys)
        self.assertIn(("use", "crate::module::Export"), keys)
        self.assertIn(("field", "Generic::field"), keys)
        self.assertIn(("field", "Wrapper::0"), keys)
        self.assertIn(("static", "MUTABLE"), keys)
        self.assertIn(("fn", "const_unsafe"), keys)
        self.assertIn(("fn", "unsafe_const"), keys)
        self.assertIn(("fn", "outer::same"), keys)
        self.assertIn(("fn", "outer::inner::same"), keys)
        self.assertIn(("fn", "Wrapper::same"), keys)
        self.assertNotIn(("field", "Wrapper::1"), keys)
        for kind, symbol in (("struct", "Generic"), ("enum", "Example"), ("trait", "Behaviour"), ("type", "Alias"), ("const", "CONSTANT"), ("static", "STATIC"), ("mod", "nested")):
            self.assertIn((kind, symbol), keys)
        self.assertNotIn(("fn", "restricted"), keys)

    def test_impl_receiver_identity_preserves_references_and_paths(self) -> None:
        source = """
trait Trait {}
struct Foo<T>(T);
struct Bar<T>(T);
impl Trait for &Foo<u8> { pub fn method() {} }
impl Trait for &Bar<u8> { pub fn method() {} }
impl<T> crate::module::Foo<T> { pub fn inherent() {} }
impl<T> crate::module::Foo<T> { pub fn another() {} }
"""

        keys = {(item.kind, item.symbol) for item in CHECKER.scan_source("src/receivers.rs", source)}

        self.assertIn(("fn", "&Foo<u8>::method"), keys)
        self.assertIn(("fn", "&Bar<u8>::method"), keys)
        self.assertIn(("fn", "crate::module::Foo<T>::inherent"), keys)
        self.assertIn(("fn", "crate::module::Foo<T>::another"), keys)

    def test_tuple_field_takes_precedence_over_fn_declaration(self) -> None:
        source = "pub fn named() {}\npub struct Tuple(pub fn(), String);\npub struct Named { pub fn_field: String }\n"

        keys = {(item.kind, item.symbol) for item in CHECKER.scan_source("src/tuple.rs", source)}

        self.assertIn(("field", "Tuple::0"), keys)
        self.assertIn(("field", "Named::fn_field"), keys)
        self.assertIn(("fn", "named"), keys)

    def test_unresolved_base_fails_loudly(self) -> None:
        with self.assertRaisesRegex(CHECKER.CheckError, "unable to resolve.*missing-base.*PUBLIC_API_BASE_REF"):
            CHECKER.check_tree(self.root, "missing-base", allowlist_path=self.root / "scripts/public_api_allowlist.tsv")

    def test_unresolved_head_fails_loudly(self) -> None:
        with self.assertRaisesRegex(CHECKER.CheckError, "unable to resolve.*missing-head.*PUBLIC_API_HEAD_REF"):
            CHECKER.check_tree(
                self.root,
                "HEAD",
                "missing-head",
                self.root / "scripts/public_api_allowlist.tsv",
            )

    def test_explicit_committed_refs_check_new_file(self) -> None:
        base = self.revision()
        self.write("src/committed_surface.rs", "pub fn committed_addition() {}\n")
        self.commit()

        findings = CHECKER.check_tree(
            self.root,
            base,
            self.revision(),
            self.root / "scripts/public_api_allowlist.tsv",
        )

        self.assertEqual(
            [(item.path, item.kind, item.symbol) for item in findings],
            [("src/committed_surface.rs", "fn", "committed_addition")],
        )

    def test_nested_and_impl_scope_identity_is_diff_stable(self) -> None:
        self.write(
            "src/scopes.rs",
            "pub fn same() {}\nmod nested { fn same() {} }\n"
            "pub const same: usize = 1;\nmod values { const same: usize = 1; }\n"
            "pub static SAME: usize = 1;\nmod statics { static SAME: usize = 1; }\n"
            "pub type Alias = String;\nmod types { type Alias = String; }\n"
            "pub struct Wrapper;\nimpl Wrapper { fn same() {} const same: usize = 1; type Alias = String; }\n",
        )
        self.commit()
        self.write(
            "src/scopes.rs",
            "pub fn same() {}\nmod nested { pub fn same() {} }\n"
            "pub const same: usize = 1;\nmod values { pub const same: usize = 1; }\n"
            "pub static SAME: usize = 1;\nmod statics { pub static SAME: usize = 1; }\n"
            "pub type Alias = String;\nmod types { pub type Alias = String; }\n"
            "pub struct Wrapper;\nimpl Wrapper { pub fn same() {} pub const same: usize = 1; pub type Alias = String; }\n",
        )

        findings = self.check()

        self.assertEqual(
            [(item.kind, item.symbol) for item in findings],
            [
                ("const", "Wrapper::same"),
                ("const", "values::same"),
                ("fn", "Wrapper::same"),
                ("fn", "nested::same"),
                ("static", "statics::SAME"),
                ("type", "Wrapper::Alias"),
                ("type", "types::Alias"),
            ],
        )

    def test_newly_public_tuple_field_has_exact_symbol(self) -> None:
        self.write("src/tuple.rs", "pub struct Wrapper(String, String);\n")
        self.commit()
        self.write("src/tuple.rs", "pub struct Wrapper(pub String, String);\n")

        findings = self.check()

        self.assertEqual(
            [(item.kind, item.symbol) for item in findings],
            [("field", "Wrapper::0")],
        )

    def test_zero_base_uses_empty_tree(self) -> None:
        self.write("src/first_commit.rs", "pub fn first_commit() {}\n")
        self.commit()

        findings = CHECKER.check_tree(
            self.root,
            "0" * 40,
            self.revision(),
            self.root / "scripts/public_api_allowlist.tsv",
        )

        self.assertEqual([(item.symbol) for item in findings], ["first_commit"])

    def test_rename_and_delete_do_not_create_additions(self) -> None:
        self.write("src/old_surface.rs", "pub fn historical() {}\n")
        self.commit()
        self.git("mv", "src/old_surface.rs", "src/renamed_surface.rs")
        self.assertEqual(self.check(), [])
        (self.root / "src/renamed_surface.rs").unlink()

        self.assertEqual(self.check(), [])

    def test_nested_comments_and_macro_bodies_are_ignored(self) -> None:
        source = """
/* outer comment
   /* nested comment with pub fn hidden() {} */
   pub fn also_hidden() {}
*/
macro_rules! generated {
    () => { pub fn macro_hidden() {} };
}
macro_rules! parenthesized {
    ($name:ident) => (pub fn macro_hidden_two() {});
}
pub fn visible() {}
"""

        keys = {item.key() for item in CHECKER.scan_source("src/comments.rs", source)}

        self.assertIn(("src/comments.rs", "fn", "visible"), keys)
        self.assertNotIn(("src/comments.rs", "fn", "hidden"), keys)
        self.assertNotIn(("src/comments.rs", "fn", "also_hidden"), keys)
        self.assertNotIn(("src/comments.rs", "fn", "macro_hidden"), keys)
        self.assertNotIn(("src/comments.rs", "fn", "macro_hidden_two"), keys)

    def test_formatting_and_order_do_not_change_classification(self) -> None:
        self.write("src/forms.rs", "pub fn first() {}\npub struct Second;\n")
        self.commit()
        self.write("src/forms.rs", "pub   struct Second { }\n\npub\nfn first ( ) { }\n")

        self.assertEqual(self.check(), [])

    def test_lifetimes_do_not_hide_later_declarations(self) -> None:
        items = CHECKER.scan_source("src/forms.rs", "fn borrow<'a>(value: &'a str) {}\npub fn visible() {}\n")

        self.assertIn(("src/forms.rs", "fn", "visible"), {item.key() for item in items})

    def test_named_lifetimes_are_single_tokens_but_char_literals_are_skipped(self) -> None:
        values = [item.value for item in CHECKER.tokens("fn borrow<'a>(value: &'a str) { let _ = 'x'; }")]

        self.assertIn("'a", values)
        self.assertNotIn("'", values)
        self.assertNotIn("x", values)

    def test_lifetime_name_only_change_is_not_a_public_addition(self) -> None:
        self.write("src/lifetime.rs", "struct Foo;\ntrait Trait {}\nimpl<'a> Trait for &'a Foo { pub fn method() {} }\n")
        self.commit()
        self.write("src/lifetime.rs", "struct Foo;\ntrait Trait {}\nimpl<'b> Trait for &'b Foo { pub fn method() {} }\n")

        self.assertEqual(self.check(), [])

    def test_impl_receiver_normalization_omits_complete_lifetime_tokens(self) -> None:
        self.assertEqual(
            CHECKER._normalize_receiver(["&", "'a", "crate", "::", "Foo"]),
            "&crate::Foo",
        )
        self.assertEqual(
            CHECKER._normalize_receiver(["&", "'b", "crate", "::", "Foo"]),
            "&crate::Foo",
        )


if __name__ == "__main__":
    unittest.main()
