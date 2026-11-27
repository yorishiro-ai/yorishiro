# Tests

## Test commands

Run commands from the repository root, or use `make -C /path/to/repo <target>`.

| Command | What it does | Backend |
|---|---|---|
| `make test-sqlite` | Full suite; `EDITION=ce` selects community, and `ee` is the default | SQLite |
| `make test-valkey` | Valkey queue tests; override the default Compose endpoint with `VALKEY_URL` | SQLite + Valkey |
| `make test-postgres` | Full suite; request tests use disposable databases | PostgreSQL |
| `make check-ce` | `cargo check --locked --no-default-features --features community --workspace` | CE |
| `make check-ee` | `cargo check --locked --features enterprise --workspace` | EE |
| `make clippy-ce` | Strict Clippy for every CE target, including tests | CE |
| `make clippy-ee` | Strict Clippy for every EE target, including tests | EE |
| `make fmt-check` | `cargo fmt --all -- --check` | Both |
| `make check-all` | Formatting, Python/boundary checks, CE check, and CE Clippy | CE |
| `make check-all-ee` | Formatting, Python/boundary checks, EE check, and EE Clippy | EE |
| `make build-ce` | Community debug build; see `Makefile` for feature flags | CE |
| `make build-ee` | `cargo build --locked --features enterprise --workspace` | EE |
| `make entities` | Spin up pgvector container, migrate, generate entities, tear down | PostgreSQL |

### Environment variables

| Variable | Default | Overrides |
|---|---|---|
| `DATABASE_URL` | `postgres://yorishiro:yorishiro@localhost:15432/yorishiro` | Target database for the test command |
| `DB_MAX_CONNECTIONS` | `10` on SQLite, `100` on PostgreSQL | Application and SQL queue pool maximum |
| `DB_CONNECT_TIMEOUT` | `500` | Connection timeout in milliseconds |

`DATABASE_URL` selects the database backend, and Cargo features select the edition.
A Valkey (Redis-compatible) queue runs with `YORISHIRO_QUEUE_KIND=Redis` and `QUEUE_URL` naming a server; the Valkey tests skip otherwise.
A test that needs a licensed edition calls `crate::requests::require_licence(&ctx)` after boot.
It installs a test licence unless `YORISHIRO_LICENSE_KEY` already holds a valid key, so these tests run in every environment.
Only the signature check needs the real key: `make test-licence` runs it, and the CI job `ee-licence` supplies the key.

Custom PostgreSQL:

```sh
DATABASE_URL=postgres://user:pass@host:5432/db make test-postgres
```

### `make entities`

```sh
make entities
```

Uses an isolated disposable pgvector container on a dynamically assigned port, waits for readiness, migrates, regenerates `src/models/_entities/*.rs`, and removes the container and volume.
Development databases are not used; entity generation requires PostgreSQL.

## Test structure

### Multi-tenant tests

Use `boot_multitenant_request` for tests that create multiple tenants through the API on PostgreSQL.
It sets the deployment cap to unlimited for that boot and installs a test licence if needed.
Community Edition and unlicensed Enterprise enforce a one-tenant deployment cap.

```rust
crate::requests::boot_multitenant_request(|request, ctx| async move {
    // Exercise multiple tenants.
}).await;
```

A test that must observe the unlicensed cap boots with `crate::requests::unlicensed()`, so the gate never sees a key.
A test changes the booted app's configuration by passing overrides to `boot_request_with(&[crate::requests::max_tenants(1)], ...)`; each override is a dotted path into `Config` such as `queue.num_workers`, applied to that one boot; the process environment is touched only by tests of configuration loading itself, which take `serial(process_environment)`.

### CI

CI runs `make test-sqlite` and `make test-postgres` for `ce` and `ee`, plus `make test-licence`, which verifies a signed key with the repository secret.
The signed licence job runs on pushes to `develop` and eligible same-repository pull requests, not on forks or merge queue refs.
Use backend guards only for tests that exercise behaviour specific to one database.
CI builds the native test harness and CLI once per edition and runs them with the same settings as the Make targets.
Codecov reports coverage for both editions and databases on pull requests and pushes to `develop`.
The package and Docker installability checks run in `packaging.yml`, selected by its path filters on pull requests and on pushes to `develop`.

The integration tests form one Loco test binary rooted at `tests/mod.rs`, without `#[path]` includes.
A test file sits at the same path as the source file it covers; not every source file has tests.

- `tests/controllers/`: REST handlers, MCP tools, middleware, route inventory, OpenAPI assembly
- `tests/data/`: configuration loading and validation (`config/`), typed settings, time, built-in templates
- `tests/db.rs`, `tests/db/`: connection primitives and migrations
- `tests/initializers/`: process-lifetime initializers
- `tests/models/`: one file per table module, with sub-modules for backend-specific or sub-module coverage (`entity_entities/sqlite.rs`, `schema_schemas/metaschema/`)
- `tests/requests/`: shared `boot_request` harness, queue/job harness (`jobs.rs`), and public-path fixtures
- `tests/services/`: embedding providers
- `tests/tasks/`: task commands, named after the task they run
- `tests/workers/`: embedding sync, reindex, queue, dispatch, lifecycle, targets
- `tests/integration/`: real child processes
- `tests/ee/`: mirrors `ee/`, including REST and MCP tests under `ee/controllers/`; compiled only with the `enterprise` feature

There is no `tests/lib.rs` and no `tests/test_helpers.rs`, and no Python test suite: Python only runs the repository checks under `scripts/`.

## Where tests live

Every test is under `tests/`. `src/` and `ee/` contain no `#[test]` and no `#[cfg(test)]`: CI rejects both.
Keep items private or crate-visible unless production callers or integration tests need the public boundary.
Do not add test-only seams to production code.
To replace a dispatcher, insert an `Arc<dyn Trait>` into `AppContext::shared_store` as production code does.

`tests/` builds without the enterprise feature (`--no-default-features --features community`).
A module that needs `ee/` is declared with `#[cfg(feature = "enterprise")]`, and a test that asserts an enterprise-only behaviour is gated the same way.

## Lints and rust-analyzer

`Cargo.toml` (`[workspace.lints]`) sets the lint levels for the Rust compiler and Clippy.
Clippy enables `all`, `pedantic`, and `nursery`, plus selected restriction lints, with its settings and banned APIs in `clippy.toml`.
The generated SeaORM entities are exempt at their module declaration.
`rust-analyzer.toml` runs Clippy on every target with the `enterprise` feature; `make clippy-ce` checks the community-only build separately.
CI and `make clippy-ce` / `make clippy-ee` add `-D warnings`, so a warning in the editor is an error in CI.
Do not dismiss an editor diagnostic because a narrower command omits its target.

- Suppress a lint with `#[expect(lint, reason = "...")]`, never a bare `#[allow]`: a stale `expect` is reported.
- `std::env::var*` is banned outside `src/data/env.rs`; Loco's `get_env` in `config/*.yaml` is the way to read settings.
- `debug_assert*`, `dbg!`, `todo!` and `unimplemented!` are rejected by the build.
- New settings belong in these files, not in command-line flags.

## SQLite gate

`tests/mod.rs` exports centralized `require_sqlite_backend()` and `require_postgres_backend()` gates.
Call sites:

```rust
if !require_sqlite_backend() { return; }
```

## `close_app_pools` (required)

`boot_request` gives each test a disposable SQLite file or PostgreSQL database and closes its pools before cleanup.
If a test boots an app another way, call `close_app_pools` from `tests/requests/mod.rs` before returning so cleanup can drop the database.

## Boot failure diagnostics

A `DROP DATABASE` panic can mask a boot failure if a pool still holds a session.
Check the boot error and ensure teardown closes the pools.

## PostgreSQL requirements

Loco creates each throwaway database with `CREATE DATABASE`, which copies `template1`. A fresh Postgres volume needs `vector` installed into `template1` itself. Verify:

```sh
psql -d template1 -c '\dx'
```

A non-superuser role is required. A superuser bypasses RLS regardless of `FORCE`.

Provider routing tests use the request harness's owned throwaway database rather than clearing the database named by `DATABASE_URL` or `QUEUE_URL`.
Process-local serial locks do not isolate separate CE and EE test processes.

Queue tests (`tests/workers/queue/`) use separate temporary SQLite application and queue files for provider-specific inspection.
Teardown drains workers and closes queue and application pools before removing those files.

## Gate testing

**A gate is not a gate until a deliberate violation makes it fire.**
A test that claims a lock or constraint holds must trigger the violation, not only replay the accepted path.
There are no barrier-based race tests: assert the observable end state of the guarded operation instead.

## MCP tools

Adding or removing an MCP tool changes the tool list. Tests that enumerate tools or build dummy arguments per tool name need updating.
