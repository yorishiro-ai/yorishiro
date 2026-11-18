# Tests

## Test commands

All commands run from the repository root. `make -C . <target>` works from any directory (the Makefile sets CWD so Loco tasks find `config/`).

| Command | What it does | Backend |
|---|---|---|
| `make test-sqlite` | The whole suite on SQLite (`cargo test`); `EDITION=ce` selects community, the default is `ee` | SQLite |
| `make test-valkey` | Only the Valkey queue provider tests, with `YORISHIRO_QUEUE_KIND=Redis` and `QUEUE_URL` (default: the compose `redis` service) | SQLite + Valkey |
| `make test-postgres` | The whole suite on PostgreSQL; every test creates and drops its own database | PostgreSQL |
| `make check-ce` | `cargo check --locked --no-default-features --workspace` | CE |
| `make check-ee` | `cargo check --locked --features enterprise --workspace` | EE |
| `make clippy-ce` | Clippy for every CE target, including public error and panic docs | CE |
| `make clippy-ee` | Clippy for every EE target, including public error and panic docs | EE |
| `make fmt-check` | `cargo fmt --all -- --check` | — |
| `make check-all` | Formatting, Python/boundary checks, CE check, and CE Clippy | CE |
| `make check-all-ee` | Formatting, Python/boundary checks, EE check, and EE Clippy | EE |
| `make build-ce` | `cargo build --locked --no-default-features --workspace` | CE |
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
Licensed tests skip unless `YORISHIRO_LICENSE_KEY` holds a valid key; the CI job `ee-postgres-licensed` supplies it.
The pinned E5 model tests skip unless `models/multilingual-e5-base/` holds the model files.
`UPDATE_FIXTURES=1 cargo test --features enterprise enterprise_inventory_contract` rewrites `tests/fixtures/mcp-inventory.md`; review the diff.

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

1. Starts `docker compose up -d testdb` (pgvector:pg18 on port 15432).
2. Waits 10 seconds for startup.
3. Runs `yorishiro db migrate` against the container.
4. Regenerates `src/models/_entities/*.rs`.
5. Tears down the container and volume.

**SQLite is not supported.** `entities` requires PostgreSQL because `cargo loco db entities` introspects the database schema, which differs between backends. Guard in the Makefile:

```sh
@if echo '$(DATABASE_URL)' | grep -q '^sqlite://'; then echo "ERROR: entities requires PostgreSQL" >&2; exit 1; fi
```

## Test structure

### Multi-tenant tests

Community Edition and unlicensed Enterprise enforce a one-tenant deployment cap.
A test that creates more than one tenant in one boot therefore only passes on a licensed Enterprise deployment: gate it after boot, so it runs in CI's `ee-postgres-licensed` job and skips everywhere else.

```rust
boot_request::<App, _, _>(|request, ctx| async move {
    if super::skip_unlicensed(&ctx) {
        return;
    }
```

Outside `tests/requests/`, call it as `crate::requests::skip_unlicensed(&ctx)`.
A test that must observe the unlicensed cap instead captures and removes `YORISHIRO_LICENSE_KEY` before boot, so the gate never sees a key.

### CI

CI runs `make test-sqlite` and `make test-postgres` for `ce` and `ee`, plus a licensed EE PostgreSQL job.
Each job runs every test; nothing is assigned or inventoried per test, so a new test needs no registration.
The real-binary REST/MCP/CLI contracts live in `tests/blackbox/` and use owned SQLite processes.
Keep deterministic fault/race fixtures internal when their guarantees need direct state control.

The integration tests are one binary rooted at `tests/mod.rs`:

- `tests/licence.rs` — licence verification, expiry boundaries
- `tests/config/`, `tests/data/` — configuration loading and validation, typed settings, built-in templates
- `tests/controllers/` — MCP tool routing, middleware, route inventory, OpenAPI assembly
- `tests/db.rs` — connection primitives
- `tests/initializers/` — process-lifetime initializers (`db_load_guard`)
- `tests/metaschema/` — nesting, projection, validation, type resolution, versioning
- `tests/models/` — entity CRUD, search, tenancy, templates, API keys, recall, queue lifecycle, the `db_enum!` conversions
- `tests/requests/` — auth, schemas, workspaces, entities, search, OAuth, Stripe, marketplace, dashboard, embedding, import, queue, etc.
- `tests/services/` — embedding providers
- `tests/tasks/` — task commands, named after the task they run
- `tests/workers/` — embedding sync, reindex, queue, dispatch, lifecycle
- `tests/ee/` — mirrors `ee/`; compiled only with the `enterprise` feature

There is no `tests/lib.rs` and no `tests/test_helpers.rs`.

## Where tests live

Every test is under `tests/`. `src/` and `ee/` contain no `#[test]` and no `#[cfg(test)]`: CI rejects both.
Keep items private or crate-visible unless production callers or integration tests need the public boundary.
Test-only seams in production code are not allowed either: a dispatcher is replaced by inserting an `Arc<dyn Trait>` into `AppContext::shared_store`, the same call production makes.

`tests/` builds without the enterprise feature (`--no-default-features --features community`).
A module that needs `ee/` is declared with `#[cfg(feature = "enterprise")]`, and a test that asserts an enterprise-only behaviour is gated the same way.

## Lints and rust-analyzer

Lint levels live in `Cargo.toml` (`[workspace.lints]`) and their settings and banned APIs in `clippy.toml`, so `cargo build`, `cargo clippy` and rust-analyzer report the same findings; `rust-analyzer.toml` makes the editor run Clippy on every target with the `enterprise` feature.
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
- `tests/models/content_entities_sqlite.rs` — entity CRUD seeded via raw SQL
- `tests/models/tenancy_sqlite.rs` — the SQLite tenancy cap
- `tests/models/search.rs` — PostgreSQL-only query shapes, guarded the other way


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

Queue tests (`tests/requests/queue.rs`) use separate temporary SQLite application and queue files for provider-specific inspection.
Teardown drains workers and closes queue and application pools before removing those files.

## Gate testing

**A gate is not a gate until a deliberate violation makes it fire.** Race-safety claims (quota locks, version-serialization locks, foreign-key-violation branches) need racing calls behind a barrier, not a sequential replay-rejection test.

## MCP tools

Adding or removing an MCP tool changes the tool list. Tests that enumerate tools or build dummy arguments per tool name need updating.
