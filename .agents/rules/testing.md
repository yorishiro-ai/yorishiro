# Tests

## Test commands

Run commands from the repository root, or use `make -C /path/to/repo <target>`.

| Command | What it does | Backend |
|---|---|---|
| `make test-sqlite` | The whole suite on SQLite (`cargo test`); `EDITION=ce` selects community, the default is `ee` | SQLite |
| `make test-valkey` | Only the Valkey queue provider tests, with `YORISHIRO_QUEUE_KIND=Redis` and `QUEUE_URL` (default: the compose `redis` service) | SQLite + Valkey |
| `make test-postgres` | The whole suite on PostgreSQL; every test creates and drops its own database | PostgreSQL |
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
| `DB_MAX_CONNECTIONS` | `100` in CI and the PostgreSQL example | PostgreSQL pool maximum; production defaults are documented separately |
| `DB_CONNECT_TIMEOUT` | `5000` in CI and the PostgreSQL example | PostgreSQL connection timeout |
The backend comes from `DATABASE_URL` and the edition from the feature set; there is no other test selector.
A Valkey (Redis-compatible) queue runs with `YORISHIRO_QUEUE_KIND=Redis` and `QUEUE_URL` naming a server; the Valkey tests skip otherwise.
A test that needs a licensed edition calls `crate::requests::require_licence(&ctx)` after boot.
It installs a test licence unless `YORISHIRO_LICENSE_KEY` already holds a valid key, so these tests run in every environment.
Only the signature check needs the real key: `make test-licence` runs it, and the CI job `ee-licence` supplies the key.
The pinned E5 model tests skip unless `models/multilingual-e5-base/` holds the model files.

Custom PostgreSQL:

```sh
DATABASE_URL=postgres://user:pass@host:5432/db \
  DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 \
  make test-postgres
```

### `make entities`

```sh
make entities
```

Uses an isolated disposable pgvector container on a dynamically assigned port, waits for readiness, migrates, regenerates `src/models/_entities/*.rs`, and removes the container and volume.
Development databases are not used; entity generation requires PostgreSQL.

## Test structure

### Multi-tenant tests

Community Edition and unlicensed Enterprise enforce a one-tenant deployment cap.
A test that creates more than one tenant in one boot therefore needs a licensed Enterprise deployment: call `require_licence` after boot.

```rust
boot_request::<App, _, _>(|request, ctx| async move {
    crate::requests::require_licence(&ctx);
```

A test that must observe the unlicensed cap boots with `crate::requests::unlicensed()`, so the gate never sees a key.
A test changes the booted app's configuration by passing overrides to `boot_request_with(&[crate::requests::max_tenants(1)], ...)`; each override is a dotted path into `Config` such as `queue.num_workers`, applied to that one boot; the process environment is touched only by tests of configuration loading itself, which take `serial(process_environment)`.

### CI

CI runs `make test-sqlite` and `make test-postgres` for `ce` and `ee`, plus `make test-licence`, which verifies a signed key with the repository secret.
Each job runs every test; nothing is assigned or inventoried per test, so a new test needs no registration.
The real-binary REST/MCP/CLI contracts live in `tests/blackbox/`; common suites own SQLite processes and the licensed tenant suite owns a PostgreSQL database.
Keep deterministic fault/race fixtures internal when their guarantees need direct state control.
The package and Docker installability checks run in `packaging.yml`, selected by its path filters on pull requests and on pushes to `develop`.

The integration tests are one binary rooted at `tests/mod.rs`.
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

There is no `tests/lib.rs` and no `tests/test_helpers.rs`.

## Where tests live

Every test is under `tests/`. `src/` and `ee/` contain no `#[test]` and no `#[cfg(test)]`: CI rejects both.
Keep items private or crate-visible unless production callers or integration tests need the public boundary.
Test-only seams in production code are not allowed either: a dispatcher is replaced by inserting an `Arc<dyn Trait>` into `AppContext::shared_store`, the same call production makes.

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

Files that need a specific backend guard with it:
- `tests/models/content_entities_sqlite.rs`: entity CRUD seeded via raw SQL
- `tests/models/tenancy_sqlite.rs`: the SQLite tenancy cap
- `tests/models/search.rs`: PostgreSQL-only query shapes, guarded the other way


## `close_app_pools` (required)

**`boot_request` closes the pools itself; a test that boots an app by other means must call `close_app_pools` before it returns, or teardown panics even on a passing test.**

One definition, in `tests/requests/mod.rs`. One crate means one binary, so every suite reaches it by path.

`after_context` shares the application pool with identity operations and opens a separate tenant pool.
Test teardown must explicitly close both pools before `DROP DATABASE`.

## Boot failure diagnostics

A boot failure that leaves a pool open surfaces as a `DROP DATABASE` panic during cleanup, not as the actual boot error. `after_context` opens the identity pool eagerly before `db::converge` runs; if `converge` then fails, that pool holds a session that blocks `DROP DATABASE`.

`close_app_pools` in `tests/requests/mod.rs` closes every pool this app opens. Use it before assuming a `DROP DATABASE` panic is the actual failure.

## PostgreSQL requirements

Loco creates each throwaway database with `CREATE DATABASE`, which copies `template1`. A fresh Postgres volume needs `vector`/`pg_trgm` installed into `template1` itself. Verify:

```sh
psql -d template1 -c '\dx'
```

A non-superuser role is required. A superuser bypasses RLS regardless of `FORCE`.

Provider routing tests use the request harness's owned throwaway database rather than clearing the database named by `DATABASE_URL` or `QUEUE_URL`.
Process-local serial locks do not isolate separate CE and EE test processes.

Queue tests (`tests/workers/queue/`) use separate temporary SQLite application and queue files for provider-specific inspection.
Teardown drains workers and closes queue and application pools before removing those files.

## Gate testing

**A gate is not a gate until a deliberate violation makes it fire.** Race-safety claims (quota locks, version-serialization locks, foreign-key-violation branches) need racing calls behind a barrier, not a sequential replay-rejection test.

## MCP tools

Adding or removing an MCP tool changes the tool list. Tests that enumerate tools or build dummy arguments per tool name need updating.
