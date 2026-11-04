#!/usr/bin/env python3
"""Inventory guard self-checks using synthetic Cargo messages and fake libtest listings only."""

import copy
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("test_assignments_guard", Path(__file__).resolve().parents[2] / "scripts/test_assignments.py")
guard = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(guard)


class InventoryGuardTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        files = {
            "tests/mod.rs": 'mod common;\n#[cfg(feature = "enterprise")]\nmod ee;\n',
            "tests/common.rs": '#[test]\nfn ordinary() {}\n#[test]\nfn shared_delta() {}\nfn helper() {}\n',
            "tests/ee.rs": '#[test]\nfn extra() {}\n',
            "src/lib.rs": "",
            "Cargo.toml": "[package]\nname='fake'\n",
            "migration/Cargo.toml": "[package]\nname='migration'\n",
            "migration/src/lib.rs": "",
            "Cargo.lock": "# synthetic lock\n",
            ".cargo/config.toml": "# synthetic config\n",
            "patches/loco-rs/fixes.patch": "synthetic framework patch\n",
            "patches/loco-rs/verify.py": "# pinned upstream checksum\n",
            "config/test_sqlite.yaml": "database: sqlite\n",
            "tests/fixtures/example.json": "{}\n",
            "fake-test-executable": "never executed",
        }
        for name, content in files.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        self.manifest = {
            "targets": [
                {"package": "yorishiro", "target": "mod", "kind": "test", "source": "tests/mod.rs", "allow_zero": False},
                {"package": "yorishiro", "target": "yorishiro", "kind": "lib", "source": "src/lib.rs", "allow_zero": True},
                {"package": "migration", "target": "migration", "kind": "lib", "source": "migration/src/lib.rs", "allow_zero": True},
            ],
            "source_guard": guard.source_census(self.root),
            "common_modules": [
                {"module": "common", "source": "tests/common.rs"},
                {"module": "ee", "source": "tests/ee.rs"},
            ],
            "exceptions": [],
            "ee_delta": [{"name": "common::shared_delta", "jobs": ["common-sqlite"], "reason": "EE cfg assertions"}],
            "jobs": {"common-sqlite": {"database": "sqlite", "queue": "sqlite"}, "resource": {"status": "required-external-not-executed"}},
        }
        self.ce = self.inventory("ce", ["common::ordinary", "common::shared_delta"])
        self.ee = self.inventory("ee", ["common::ordinary", "common::shared_delta", "ee::extra"])

    def inventory(self, edition, names):
        records = []
        for target in self.manifest["targets"]:
            records.append({"package": target["package"], "target": target["target"], "kind": target["kind"],
                            "package_id": "path+file:///synthetic#yorishiro@1.0.0", "executable": "/synthetic/unused",
                            "features": ["community"] + (["enterprise"] if edition == "ee" else []),
                            "sha256": "0" * 64, "profile": {"test": True},
                            "tests": names[:] if target["kind"] == "test" else [], "ignored": []})
        return {"schema": 1, "edition": edition, "revision": "synthetic-revision", "rustc": "synthetic-rustc", "target_triple": "synthetic-host",
                "source_guard": self.manifest["source_guard"],
                "manifest_sha256": guard.digest(json.dumps(self.manifest, sort_keys=True).encode()), "artifacts": records}

    def sync_manifest_hash(self):
        for inventory in (self.ce, self.ee):
            inventory["manifest_sha256"] = guard.digest(json.dumps(self.manifest, sort_keys=True).encode())

    def verify(self):
        return guard.verify(self.ce, self.ee, self.manifest, self.root)

    def rejects(self, message):
        with self.assertRaisesRegex(ValueError, message):
            self.verify()

    def exact(self, name="common::ordinary", **values):
        return {"name": name, "source": "tests/common.rs", "editions": ["ce", "ee"],
                "role": "common", "jobs": ["common-sqlite"], "reason": "reviewed exact fixture", **values}

    def test_complete_inventory_is_not_execution_coverage(self):
        report = self.verify()
        self.assertTrue(report["inventory_valid"])
        self.assertTrue(report["execution_matrix_ready"])
        self.assertNotIn("execution_receipts", report)
        self.assertEqual(report["manifest_sha256"], self.ce["manifest_sha256"])
        self.assertTrue(report["execution_matrix"]["include"])
        owners = [(r["name"], r["owner"], r["role"]) for r in report["assignments"]]
        self.assertIn(("common::ordinary", "ce", "common"), owners)
        self.assertIn(("ee::extra", "ee", "common"), owners)
        self.assertIn(("common::shared_delta", "ee", "ee_delta"), owners)

    def test_patch_and_fixture_changes_require_ownership_review(self):
        for name in ("patches/loco-rs/fixes.patch", "patches/loco-rs/verify.py",
                     "config/test_sqlite.yaml", "tests/fixtures/example.json"):
            path = self.root / name
            original = path.read_text()
            with self.subTest(path=name):
                path.write_text(original + "\nchanged\n")
                self.rejects("source guard changed")
            path.write_text(original)

    def test_only_reviewed_e5_contracts_wait_for_model_cache(self):
        self.manifest["jobs"]["resource-e5"] = {"database": "sqlite", "queue": "sqlite", "requires_e5_cache": True}
        self.manifest["exceptions"] = [self.exact(role="resource", jobs=["resource-e5"])]
        self.sync_manifest_hash()
        report = self.verify()
        self.assertEqual([row["job"] for row in report["e5_matrix"]["include"]], ["resource-e5"])
        self.assertTrue(all(row["job"] == "common-sqlite" for row in report["core_matrix"]["include"]))
        self.assertCountEqual(report["execution_matrix"]["include"], report["core_matrix"]["include"] + report["e5_matrix"]["include"])
        self.manifest["jobs"]["resource-e5"]["requires_e5_cache"] = False
        self.sync_manifest_hash()
        self.rejects("E5 contract lacks cache requirement")

    def test_common_contract_cannot_gain_an_unreviewed_model_dependency(self):
        self.manifest["jobs"]["common-sqlite"]["requires_e5_cache"] = True
        self.sync_manifest_hash()
        self.rejects("unreviewed E5 cache consumer")

    def test_missing_ee_counterpart(self):
        self.ee["artifacts"][0]["tests"].remove("common::ordinary")
        self.rejects("missing source-declared test")

    def test_missing_from_both_inventories(self):
        for inventory in (self.ce, self.ee):
            inventory["artifacts"][0]["tests"].remove("common::ordinary")
        self.rejects("missing source-declared test")

    def test_wrong_inventory_edition(self):
        self.ee["edition"] = "ce"
        self.rejects("wrong inventory edition")

    def test_wrong_artifact_features(self):
        self.ce["artifacts"][0]["features"].append("enterprise")
        self.rejects("wrong artifact edition")

    def test_unknown_compiled_identity_under_common_prefix(self):
        self.ce["artifacts"][0]["tests"].append("common::unknown")
        self.rejects("unknown compiled identity")

    def test_helper_function_is_not_a_test(self):
        self.ce["artifacts"][0]["tests"].append("common::helper")
        self.rejects("unknown compiled identity")

    def test_parent_module_cannot_absorb_new_child(self):
        self.ce["artifacts"][0]["tests"].append("common::child::unknown")
        self.rejects("unknown compiled identity")

    def test_stale_exact_identity(self):
        self.manifest["exceptions"].append(self.exact("common::typo"))
        self.sync_manifest_hash()
        self.rejects("stale exact")

    def test_wrong_exact_edition(self):
        self.manifest["exceptions"].append(self.exact(editions=["ee"]))
        self.sync_manifest_hash()
        self.rejects("missing/wrong edition for exact")

    def test_ambiguous_exact_assignment(self):
        self.manifest["exceptions"] = [self.exact(), self.exact()]
        self.sync_manifest_hash()
        self.rejects("duplicate exact")

    def test_ambiguous_common_rule(self):
        self.manifest["common_modules"].append(copy.deepcopy(self.manifest["common_modules"][0]))
        self.sync_manifest_hash()
        self.rejects("ambiguous common")

    def test_unclassified_ignored_identity(self):
        self.ce["artifacts"][0]["ignored"] = ["common::ordinary"]
        self.rejects("unclassified ignored")

    def test_known_external_resource_requires_no_credentials_to_classify(self):
        self.manifest["exceptions"].append(self.exact(role="resource", jobs=["resource"], include_ignored=True))
        self.sync_manifest_hash()
        for inventory in (self.ce, self.ee):
            inventory["artifacts"][0]["ignored"] = ["common::ordinary"]
        report = self.verify()
        self.assertEqual(report["external_contracts"][0]["status"], "required-execution-receipt")

    def test_mixed_contract_is_pending_not_guard_failure(self):
        self.manifest["exceptions"].append(self.exact(role="pending_split", jobs=[]))
        self.sync_manifest_hash()
        report = self.verify()
        self.assertTrue(report["inventory_valid"])
        self.assertEqual(len(report["pending_splits"]), 1)
        self.assertFalse(report["execution_matrix_ready"])

    def test_helper_has_explicit_parent_role(self):
        self.manifest["exceptions"].append(self.exact(role="helper", jobs=[], parent="common::shared_delta"))
        self.sync_manifest_hash()
        self.assertTrue(self.verify()["helpers"][0]["requires_parent_child_receipt"])

    def test_missing_helper_parent(self):
        self.manifest["exceptions"].append(self.exact(role="helper", jobs=[], parent="common::missing"))
        self.sync_manifest_hash()
        self.rejects("missing helper parent")

    def test_duplicate_artifact(self):
        self.ce["artifacts"].append(copy.deepcopy(self.ce["artifacts"][0]))
        self.rejects("unknown/duplicate artifact")

    def test_duplicate_compiled_name(self):
        self.ce["artifacts"][0]["tests"].append("common::ordinary")
        self.rejects("duplicate compiled identity")

    def test_missing_zero_test_artifact_is_still_fatal(self):
        self.ce["artifacts"].pop()
        self.rejects("missing workspace test artifact")

    def test_nonzero_library_cannot_disappear(self):
        self.ce["artifacts"][1]["tests"] = ["new_library_test"]
        self.rejects("unknown compiled identity")

    def test_nonzero_unit_and_migration_can_have_exact_reviewed_owners(self):
        for index in (1, 2):
            target = self.manifest["targets"][index]
            (self.root / target["source"]).write_text("#[test]\nfn owned() {}\n")
            self.manifest["exceptions"].append(self.exact(
                "owned", package=target["package"], target=target["target"], kind="lib", source=target["source"]))
            for inventory in (self.ce, self.ee):
                inventory["artifacts"][index]["tests"] = ["owned"]
        self.manifest["source_guard"] = guard.source_census(self.root)
        for inventory in (self.ce, self.ee):
            inventory["source_guard"] = self.manifest["source_guard"]
        self.sync_manifest_hash()
        report = self.verify()
        self.assertEqual(sum(row["name"] == "owned" for row in report["assignments"]), 2)

    def test_zero_integration_artifact_is_fatal(self):
        self.ce["artifacts"][0]["tests"] = []
        self.rejects("unexpected zero-test artifact")

    def test_source_guard_rejects_new_direct_and_indirect_edges(self):
        path = self.root / "tests/common.rs"
        original = path.read_text()
        for change in ("fn new_helper() { require_postgres_backend(); }",
                       "fn indirect() { postgres_test_url(); }",
                       "fn provider() { if queue_kind() { return; } }"):
            with self.subTest(change=change):
                path.write_text(original + change)
                self.rejects("source guard changed")
        path.write_text(original)

    def test_libtest_listing_is_strict(self):
        self.assertEqual(guard.parse_listing("a: test\nb: test\n"), ["a", "b"])
        self.assertEqual(guard.parse_listing(""), [])
        for text in ("a: test\na: test\n", "a: benchmark\n", "garbage\n"):
            with self.subTest(text=text), self.assertRaises(ValueError):
                guard.parse_listing(text)

    def messages(self):
        result = []
        for target in self.manifest["targets"]:
            result.append({"reason": "compiler-artifact", "profile": {"test": True},
                           "package_id": "path+file:///synthetic#yorishiro@1.0.0", "features": ["community"],
                           "executable": str(self.root / "fake-test-executable"),
                           "target": {"name": target["target"], "kind": [target["kind"]],
                                      "src_path": str(self.root / target["source"])}})
        result.append({"reason": "build-finished", "success": True})
        return result

    def test_collect_exact_actual_artifacts_including_zero(self):
        messages = "\n".join(json.dumps(message) for message in self.messages())
        calls = []

        def fake_run(argv):
            calls.append(argv)
            if "--list" in argv:
                return ""
            return "host: synthetic-host\n" if argv[0] == "rustc" else "synthetic metadata"

        result = guard.collect(messages, "ce", self.manifest, self.root, fake_run)
        self.assertEqual(len(result["artifacts"]), 3)
        self.assertEqual(result["artifacts"][0]["sha256"], guard.digest(b"never executed"))
        self.assertEqual(sum("--list" in argv for argv in calls), 6)
        self.assertEqual(sum("--ignored" in argv for argv in calls), 3)

    def test_cargo_messages_reject_missing_duplicate_or_failed_build(self):
        cases = [self.messages()[1:], self.messages()[:1] + self.messages(),
                 self.messages()[:-1] + [{"reason": "build-finished", "success": False}]]
        for messages in cases:
            with self.subTest(messages=messages), self.assertRaises(ValueError):
                guard.artifacts("\n".join(json.dumps(message) for message in messages), self.manifest, self.root)


class SharedTenantFixtureTests(unittest.TestCase):
    def test_shared_ee_postgres_fixtures_have_licensed_capacity(self):
        manifest = json.loads((Path(__file__).with_name("assignments.json")).read_text())
        expected = {
            "models::query_embedding_requests::a_disabled_tenant_cannot_publish_a_query_embedding",
            "requests::tenant_auth::a_workspace_scoped_key_still_works_on_a_base_route",
            "requests::tenant_auth::a_tenant_scoped_key_resolves_the_workspace_named_by_the_header",
            "requests::tenant_auth::tenant_key_rechecks_direct_workspace_grants_after_revocation",
            "requests::schema_forks::concurrent_opposite_forks_allow_one_edge_and_reject_the_other",
            "requests::schema_forks::fork_creation_shares_the_schema_version_lock",
            "requests::origin::publication_waits_for_merge_revision_lock_and_remains_pending",
            "requests::inference::terminal_proposal_actions_serialize_against_confirmation",
            "requests::inference::the_infer_fill_scope_carries_the_workspaces_tenant",
        }
        found = set()
        for group in manifest["exception_groups"]:
            for name in expected.intersection(group["names"]):
                with self.subTest(name=name):
                    self.assertEqual(group["editions"], ["ee"])
                    self.assertEqual(len(group["jobs"]), 1)
                    job = manifest["jobs"][group["jobs"][0]]
                    self.assertEqual(job["database"], "postgres")
                    self.assertTrue(job["signed_licence"])
                    self.assertNotIn(name, found)
                    found.add(name)
        self.assertEqual(found, expected)


if __name__ == "__main__":
    unittest.main()
