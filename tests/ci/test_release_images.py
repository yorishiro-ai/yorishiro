#!/usr/bin/env python3
"""Release image provenance guards, without accessing a registry."""

import argparse
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("release_images", Path(__file__).resolve().parents[2] / "scripts/release_images.py")
IMAGES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(IMAGES)


class ImageProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.args = argparse.Namespace(directory=Path(self.temp.name), image="ghcr.io/example/app", source="a" * 40,
                                       run_id="123", arch="amd64", edition="ce", tag="v0.100.0")
        self.args.metadata = self.args.directory / "metadata.json"
        self.metadata = {f"release-{edition}": {"containerimage.digest": "sha256:" + value * 64}
                         for edition, value in (("ce", "1"), ("ee", "2"))}
        self.args.metadata.write_text(json.dumps(self.metadata))

    def records(self):
        for arch in ("amd64", "arm64"):
            self.args.arch = arch
            IMAGES.record_build(self.args)

    def manifests(self):
        self.records()
        for edition in ("ce", "ee"):
            self.args.edition = edition
            with patch.object(IMAGES, "docker", side_effect=["", json.dumps({"digest": "sha256:" + "3" * 64})]):
                IMAGES.join(self.args)
            path = self.args.directory / f"manifest-{edition}.json"
            (self.args.directory / f"smoke-{edition}.json").write_text(path.read_text())

    def test_records_the_actual_bake_digest_for_both_editions(self):
        IMAGES.record_build(self.args)
        ce = IMAGES.load(self.args.directory / "ce-amd64.json", self.args, "ce", "amd64")
        self.assertEqual(ce["digest"], self.metadata["release-ce"]["containerimage.digest"])
        self.assertTrue((self.args.directory / "ee-amd64.json").is_file())

    def test_missing_edition_and_invalid_digest_do_not_publish_partial_records(self):
        for metadata in ({"release-ce": self.metadata["release-ce"]},
                         {**self.metadata, "release-ee": {"containerimage.digest": "mutable-tag"}}):
            self.args.metadata.write_text(json.dumps(metadata))
            with self.assertRaises((KeyError, ValueError)):
                IMAGES.record_build(self.args)
            self.assertFalse((self.args.directory / "ce-amd64.json").exists())

    def test_join_uses_only_recorded_digests_not_mutable_architecture_tags(self):
        self.records()
        with patch.object(IMAGES, "docker", side_effect=["", json.dumps({"digest": "sha256:" + "3" * 64})]) as docker:
            IMAGES.join(self.args)
        argv = docker.call_args_list[0].args
        self.assertEqual(argv[-2:], (self.args.image + "@sha256:" + "1" * 64,) * 2)
        self.assertIn(self.args.image + ":candidate-123-ce", argv)
        self.assertEqual(IMAGES.reference(self.args), self.args.image + "@sha256:" + "3" * 64)

    def test_other_run_source_image_edition_or_architecture_fails_before_docker(self):
        self.records()
        path = self.args.directory / "ce-arm64.json"
        original = json.loads(path.read_text())
        for key, value in (("run_id", "124"), ("source", "b" * 40), ("image", "ghcr.io/other/app"),
                           ("edition", "ee"), ("arch", "amd64"), ("digest", "tag")):
            with self.subTest(key=key):
                path.write_text(json.dumps({**original, key: value}))
                with patch.object(IMAGES, "docker") as docker, self.assertRaises(ValueError):
                    IMAGES.join(self.args)
                docker.assert_not_called()

    def test_missing_architecture_is_not_resolved_from_a_tag(self):
        IMAGES.record_build(self.args)
        with patch.object(IMAGES, "docker") as docker, self.assertRaises(FileNotFoundError):
            IMAGES.join(self.args)
        docker.assert_not_called()

    def test_promotion_uses_the_exact_smoked_manifest_and_architecture_digests(self):
        self.manifests()
        with patch.object(IMAGES, "docker", side_effect=["", json.dumps({"digest": "sha256:" + "3" * 64}), "", ""] * 2) as docker:
            IMAGES.promote(self.args)
        self.assertEqual(docker.call_count, 8)
        self.assertEqual(docker.call_args_list[0].args,
                         ("create", "-t", self.args.image + ":v0.100.0-ce", self.args.image + "@sha256:" + "3" * 64))
        self.assertIn(self.args.image + ":v0.100.0-ce-amd64", docker.call_args_list[2].args)

    def test_missing_or_different_smoke_receipt_cannot_promote_any_edition(self):
        self.manifests()
        path = self.args.directory / "smoke-ee.json"
        record = json.loads(path.read_text())
        path.write_text(json.dumps({**record, "digest": "sha256:" + "4" * 64}))
        with patch.object(IMAGES, "docker") as docker, self.assertRaises(ValueError):
            IMAGES.promote(self.args)
        docker.assert_not_called()
        path.unlink()
        with patch.object(IMAGES, "docker") as docker, self.assertRaises(FileNotFoundError):
            IMAGES.promote(self.args)
        docker.assert_not_called()

    def test_registry_promotion_must_preserve_the_manifest_digest(self):
        self.manifests()
        with patch.object(IMAGES, "docker", side_effect=["", json.dumps({"digest": "sha256:" + "4" * 64})]), self.assertRaises(ValueError):
            IMAGES.promote(self.args)


if __name__ == "__main__":
    unittest.main()
