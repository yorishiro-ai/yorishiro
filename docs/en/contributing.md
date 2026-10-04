# Contributing

## Tests

Use `make test-postgres` or `make test-sqlite` to run the integration suite.
Both commands keep shared tests in the run and verify that at least one test specific to the selected backend executes.
The final output reports selected, executed, and skipped backend-specific gate counts, including the reason for skipped gates.
The backend gate wrapper is `uv run scripts/test_backend.py`.
Use `make test-redis` to run the dedicated Valkey queue and configuration tests against a local or explicitly configured endpoint.
The Redis test URL must select database 15 because Loco clears that reserved database during queue cleanup.
The remaining repository helper scripts also run through the uv project and use only Python's standard library.
Tests of public behavior belong in `tests/`.
Tests may remain beside production code only when they exercise private implementation seams that should not become public solely for testing.
Test output must be limited to protocol handshakes, backend-gate reporting, and diagnostics needed to explain an intentional skip or failure.
Migration tests cover fresh installs, incremental upgrades from each recorded migration boundary, and rollback separately on both supported backends.
The PostgreSQL migration tests use unique scratch databases and the SQLite tests use temporary files, so every test tears down its own database resources.

Before pushing, run `make check-all`.
