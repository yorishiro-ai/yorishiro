# Tests

## Test commands

All commands run from the repository root. `make -C . <target>` works from any directory (the Makefile sets CWD so Loco tasks find `config/`).

| Command | What it does | Backend |
|---|---|---|
| `make test-postgres` | PostgreSQL DB and PostgreSQL queue topology | PostgreSQL/PostgreSQL |
| `make test-sqlite` | SQLite DB and SQLite queue topology using separate files | SQLite/SQLite |
| `make test-valkey` | SQLite DB and Valkey-compatible `redis://` queue topology | SQLite/Valkey |
| `uv run scripts/test_topology.py <topology>` | Full topology suite with configured-app evidence | Any supported topology |
| `make check-ce` | `cargo check --locked --no-default-features --workspace` | CE |
| `make check-ee` | `cargo check --locked --features enterprise --workspace` | EE |
| `make clippy-ce` | Clippy for every CE target, including public error and panic docs | CE |
| `make clippy-ee` | Clippy for every EE target, including public error and panic docs | EE |
| `make fmt-check` | `cargo fmt --all -- --check` | — |
| `make check-all` | Formatting, Python/public API checks, CE check, and CE Clippy | CE |
| `make check-all-ee` | Formatting, Python/public API checks, EE check, and EE Clippy | EE |
| `make build-ce` | `cargo build --locked --no-default-features --workspace` | CE |
| `make build-ee` | `cargo build --locked --features enterprise --workspace` | EE |
| `make entities` | Spin up pgvector container, migrate, generate entities, tear down | PostgreSQL |

### Environment variables

| Variable | Default | Overrides |
|---|---|---|
| `DATABASE_URL` | `postgres://yorishiro:yorishiro@localhost:15432/yorishiro` | Target database for the test command |
| `DB_MAX_CONNECTIONS` | `100` in CI and the PostgreSQL example | PostgreSQL pool maximum; production defaults are documented separately |
| `DB_CONNECT_TIMEOUT` | `5000` in CI and the PostgreSQL example | PostgreSQL connection timeout |
| `LOCO_ENV` | `test_postgres` or `test_sqlite` | Selects the test configuration |

Custom PostgreSQL:

```sh
DATABASE_URL=postgres://user:pass@host:5432/db \
  DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres \
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

The integration tests are one binary rooted at `tests/mod.rs`:

- `tests/licence.rs` — licence verification, expiry boundaries
- `tests/config/`, `tests/data/` — configuration loading and validation, typed settings, built-in templates
- `tests/controllers/` — MCP tool routing, route inventory, OpenAPI assembly
- `tests/db.rs`, `tests/db_enum.rs` — connection primitives and the `db_enum!` conversions
- `tests/initializers/` — process-lifetime initializers (`db_load_guard`)
- `tests/metaschema/` — nesting, projection, validation, type resolution, versioning
- `tests/migration/` — `postgres.rs`, `sqlite.rs`
- `tests/models/` — entity CRUD, search, tenancy, templates, API keys, recall, queue lifecycle
- `tests/requests/` — auth, schemas, workspaces, entities, search, OAuth, Stripe, marketplace, dashboard, embedding, import, queue, etc.
- `tests/services/` — embedding, rate limiting
- `tests/tasks/` — task commands (reindex_embeddings)
- `tests/workers/` — embedding sync, reindex, queue, dispatch, lifecycle
- `tests/ee/` — mirrors `ee/`; compiled only with the `enterprise` feature

There is no `tests/lib.rs` and no `tests/test_helpers.rs`.

## Where tests live

Every test is under `tests/`. `src/` and `ee/` contain no `#[test]` and no `#[cfg(test)]`: CI rejects both.
A test that needs an item the crate keeps private widens it to `pub` and records it in `scripts/public_api_allowlist.tsv` under the `integration-test` boundary.
Test-only seams in production code are not allowed either: a dispatcher is replaced by inserting an `Arc<dyn Trait>` into `AppContext::shared_store`, the same call production makes.

`tests/` builds without the enterprise feature (`--no-default-features --features community`).
A module that needs `ee/` is declared with `#[cfg(feature = "enterprise")]`, and a test that asserts an enterprise-only behaviour is gated the same way.

## rust-analyzer is authoritative

The repository Clippy commands must cover the same targets rust-analyzer checks.
Run `make clippy-ce` and `make clippy-ee`; both use `--all-targets`, deny warnings, and require `# Errors` and `# Panics` documentation on public APIs.
Do not dismiss an editor diagnostic because a narrower CI command omits its target or lint.

## SQLite gate

`tests/mod.rs` exports centralized `require_sqlite_backend()` and `require_postgres_backend()` gates.
Each gate records selected and skipped gate markers when `YORISHIRO_BACKEND_MARKER_DIR` is set.
Call sites:

```rust
if !require_sqlite_backend() { return; }
```

Dedicated SQLite files use this gate:
- `tests/models/content_entities_sqlite.rs` — entity CRUD (in-memory, seeds via raw SQL)
- `tests/models/tenancy_sqlite.rs` — tenancy cap
- `tests/models/search.rs` — vector and FTS5 fallback
- `tests/requests/auth_sqlite.rs` — authentication
- `tests/requests/schemas_sqlite.rs` — schema CRUD
- `tests/requests/workspaces_sqlite.rs` — workspace CRUD
- `tests/migration/sqlite.rs` — migration verification

`uv run scripts/test_topology.py` runs the suite with explicit CE or EE feature flags and verifies a configured-app marker emitted after `Hooks::load_config(Environment::Test)` resolves the expected database URI and `QueueConfig` variant.
The runner rejects unexpected marker directories and fails when the expected topology has no configured-app evidence.

`request_with_create_db` is not wired for SQLite (`CREATE DATABASE` has no SQLite equivalent).

## `close_app_pools` (required)

**A request test that boots through `request_with_create_db` must call `close_app_pools` before its closure returns, or teardown panics even on a passing test.**

One definition, in `tests/requests/mod.rs`. One crate means one binary, so every suite reaches it by path.

`after_context` opens the identity pool (eager) and tenant pool (lazy). `ctx.db`'s own pool is a third connection. `spawn_startup_reindex` holds a session on `ctx.db` for the process lifetime. Without signaling its `StartupReindexHandle` before closing pools, `DROP DATABASE` panics.

## Boot failure diagnostics

A boot failure inside `request_with_create_db` surfaces as a `DROP DATABASE` panic during cleanup, not as the actual boot error. `after_context` opens the identity pool eagerly before `db::converge` runs; if `converge` then fails, that pool holds a session that blocks `DROP DATABASE`.

`close_app_pools` in `tests/requests/mod.rs` closes every pool this app opens. Use it before assuming a `DROP DATABASE` panic is the actual failure.

## PostgreSQL requirements

Loco creates each throwaway database with `CREATE DATABASE`, which copies `template1`. A fresh Postgres volume needs `vector`/`pg_trgm` installed into `template1` itself. Verify:

```sh
psql -d template1 -c '\dx'
```

A non-superuser role is required. A superuser bypasses RLS regardless of `FORCE`.

Queue tests (`tests/requests/queue.rs`) bypass `request_with_create_db` because the queue pool has no public close path. They use a temporary SQLite queue file instead.

## Gate testing

**A gate is not a gate until a deliberate violation makes it fire.** Race-safety claims (quota locks, version-serialization locks, foreign-key-violation branches) need racing calls behind a barrier, not a sequential replay-rejection test.

## MCP tools

Adding or removing an MCP tool changes the tool list. Tests that enumerate tools or build dummy arguments per tool name need updating.
