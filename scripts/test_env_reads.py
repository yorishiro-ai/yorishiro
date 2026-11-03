#!/usr/bin/env python3
"""Regression checks for the runtime environment boundary."""

import unittest

from check_env_reads import violations


class EnvReadsTest(unittest.TestCase):
    def test_reads_and_aliases_are_rejected(self):
        for source in (
            'std::env::var("KEY")',
            'std :: env ::\nvar_os("KEY")',
            'use std::env as process; process::var("KEY")',
            'use std::env::{var as read}; read("KEY")',
            'use std::env::vars_os as entries;',
            'use std::env::*;',
            'use std::{env::{var as read}};',
        ):
            with self.subTest(source=source):
                self.assertTrue(violations("src/models/example.rs", source))

    def test_non_configuration_operations_are_allowed(self):
        source = '''
            std::env::args_os(); std::env::current_dir();
            std::env::temp_dir(); std::env::current_exe();
            env!("CARGO_PKG_VERSION"); option_env!("BUILD_ID");
            // std::env::var("not code")
            let text = r#"std::env::var("not code")"#;
            let vars = Vars::from_cli_args(args);
        '''
        self.assertEqual(violations("src/app.rs", source), [])

    def test_registries_and_generic_guard_are_allowed(self):
        for path in ("src/data/env.rs", "tests/env.rs", "tests/mod.rs"):
            self.assertEqual(violations(path, 'std::env::var("KEY")'), [])


if __name__ == "__main__":
    unittest.main()
