#!/usr/bin/env python3
"""Keep application environment inputs synchronized across deployment examples."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KEYS = re.compile(r'get_env\(name="(YORISHIRO_[A-Z0-9_]+)"')
# Queue kind is fixed by test topology; reaping applies only to SQL queues.
TOPOLOGY_KEYS = {"YORISHIRO_QUEUE_KIND", "YORISHIRO_QUEUE_REAPER_AGE_MINUTES"}


class EnvConfigTest(unittest.TestCase):
    def test_application_keys_are_available_in_every_config(self):
        configs = {path.name: set(KEYS.findall(path.read_text()))
                   for path in (ROOT / "config").glob("*.yaml")}
        common = set.union(*configs.values()) - TOPOLOGY_KEYS
        for name, keys in configs.items():
            with self.subTest(config=name):
                self.assertEqual(common - keys, set())

    def test_test_registry_keys_have_a_config_or_harness_owner(self):
        registry = set(re.findall(r'=> "([A-Z][A-Z0-9_]+)"', (ROOT / "tests/env.rs").read_text()))
        config_keys = set()
        for path in (ROOT / "config").glob("*.yaml"):
            config_keys.update(re.findall(r'get_env\(name="([A-Z][A-Z0-9_]+)"', path.read_text()))
        direct = set(re.findall(r'=> "([A-Z][A-Z0-9_]+)"', (ROOT / "src/data/env.rs").read_text()))
        harness = {
            "LOCO_ENV",
            "YORISHIRO_BACKEND_MARKER_DIR",
            "YORISHIRO_REQUEST_TIMING_DIR",
            "YORISHIRO_SQLITE_LOCK_CHILD_URI",
            "YORISHIRO_TEST_E5_MODEL",
            "YORISHIRO_TEST_E5_TOKENIZER",
            "YORISHIRO_TEST_REDIS_BASE_URL",
            "YORISHIRO_TOPOLOGY_MARKER_DIR",
        }
        self.assertEqual(registry - config_keys - direct - harness, set())
        self.assertLessEqual(harness, registry)


if __name__ == "__main__":
    unittest.main()
