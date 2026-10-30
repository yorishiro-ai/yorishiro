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


if __name__ == "__main__":
    unittest.main()
