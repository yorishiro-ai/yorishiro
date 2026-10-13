# Contributing

## Tests

Use `make test-postgres`, `make test-sqlite`, or `make test-valkey` for the common local topologies.
Use `uv run scripts/test_topology.py <topology>` for `postgres-postgres`, `sqlite-sqlite`, `postgres-valkey`, or `sqlite-valkey`.
The runner passes explicit CE or EE Cargo features and verifies evidence from Loco's resolved test configuration, not from environment labels.
The topology gate wrapper is `uv run scripts/test_topology.py`.
After a successful test command, rejected topology evidence emits `YORISHIRO_TOPOLOGY_REJECTED` on stderr and exits nonzero.
CI checks both conditions so build or test failures cannot masquerade as a successful mismatch self-test.
CI runs all four permitted topologies for both CE and EE, with eight distinct edition/database/queue job names.
`make entities` requires the `sea-orm-cli` version pinned in `Cargo.lock`, matching CI; use a fresh PostgreSQL database to verify generation from migrations alone.
The pinned generator omits partial unique indexes from entities, so those indexes remain owned solely by migrations.
`make python-lint` (also in both `check-all` targets) statically verifies every matrix pair appears exactly once and rejects crossed SQL pairs or extra axes.
Use `make test-valkey` to run the Valkey topology tests against a local or explicitly configured endpoint.
The Valkey-compatible `redis://` endpoint must select database 15 because Loco clears that reserved database during queue cleanup.
The remaining repository helper scripts also run through the uv project and use only Python's standard library.
Every test lives in `tests/`.
`src/`, `ee/`, `edition/`, and `migration/src/` contain no test attributes or `#[cfg(test)]`; `make test-location-check` rejects both locally and in CI.
`make debug-macro-check` rejects `dbg!` and `debug_assert*` in Rust code; use `assert!` for invariants that must hold in every build.
A test that needs an item the crate keeps private widens it to `pub` and records the item in `scripts/public_api_allowlist.tsv` under the `integration-test` boundary described in [public-api.md](public-api.md).
`tests/` also builds without the enterprise feature, so the Community edition is tested on its own: modules that need `ee/` carry `#[cfg(feature = "enterprise")]`, and `tests/ee/` mirrors `ee/`.
The feature gate belongs only in `tests/` and `edition/`: `src/` must not name the enterprise overlay at all, which `make edition-boundary-check` enforces.
Test output must be limited to protocol handshakes, topology evidence, and diagnostics needed to explain an intentional skip or failure.
Migration tests cover fresh installs, incremental upgrades from each recorded migration boundary, and rollback separately on both supported backends.
The PostgreSQL migration tests use unique scratch databases and the SQLite tests use temporary files, so every test tears down its own database resources.

Before pushing, run `make check-all`.
