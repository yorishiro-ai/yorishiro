# Contributing

## Tests

Use `make test-postgres`, `make test-sqlite`, or `make test-valkey` for the common local topologies.
Use `uv run scripts/test_topology.py <topology>` for any of the six supported database and queue combinations.
The runner passes explicit CE or EE Cargo features and verifies evidence from Loco's resolved test configuration, not from environment labels.
The topology gate wrapper is `uv run scripts/test_topology.py`.
Use `make test-valkey` to run the Valkey topology tests against a local or explicitly configured endpoint.
The Valkey-compatible `redis://` endpoint must select database 15 because Loco clears that reserved database during queue cleanup.
The remaining repository helper scripts also run through the uv project and use only Python's standard library.
Every test lives in `tests/`.
`src/`, `ee/`, and `edition/` contain no `#[test]` and no `#[cfg(test)]`, and CI rejects both.
A test that needs an item the crate keeps private widens it to `pub` and records the item in `scripts/public_api_allowlist.tsv` under the `integration-test` boundary described in [public-api.md](public-api.md).
`tests/` also builds without the enterprise feature, so the Community edition is tested on its own: modules that need `ee/` carry `#[cfg(feature = "enterprise")]`, and `tests/ee/` mirrors `ee/`.
The feature gate belongs only in `tests/` and `edition/`: `src/` must not name the enterprise overlay at all, which `make edition-boundary-check` enforces.
Test output must be limited to protocol handshakes, topology evidence, and diagnostics needed to explain an intentional skip or failure.
Migration tests cover fresh installs, incremental upgrades from each recorded migration boundary, and rollback separately on both supported backends.
The PostgreSQL migration tests use unique scratch databases and the SQLite tests use temporary files, so every test tears down its own database resources.

Before pushing, run `make check-all`.
