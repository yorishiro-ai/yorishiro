# Contributing

## Tests

Use `make test-postgres` or `make test-sqlite` to run the integration suite.
Both commands keep shared tests in the run and verify that at least one test specific to the selected backend executes.
The final output reports selected, executed, and skipped backend-specific gate counts, including the reason for skipped gates.
Migration tests cover fresh installs, incremental upgrades from each recorded migration boundary, and rollback separately on both supported backends.
The PostgreSQL migration tests use unique scratch databases and the SQLite tests use temporary files, so every test tears down its own database resources.

Before pushing, run `make check-all`.
