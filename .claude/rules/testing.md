# Tests

## Test commands

All commands run from the repository root. `make -C . <target>` works from any directory (the Makefile sets CWD so Loco tasks find `config/`).

| Command | What it does | Backend |
|---|---|---|
| `make test-postgres` | Full suite (`cargo test --locked --workspace -- --test-threads=1`) | PostgreSQL |
| `make test-sqlite` | Full suite with `require_sqlite_backend()` gate | SQLite |
| `make check` | `cargo check --locked --workspace` | — |
| `make clippy` | `cargo clippy --locked --workspace --tests -- -D warnings` | — |
| `make fmt-check` | `cargo fmt --all -- --check` | — |
| `make check-all` | `fmt-check + check + clippy` (CI check job) | — |
| `make build` | `cargo build --locked --workspace` | — |
| `make entities` | Spin up pgvector container, migrate, generate entities, tear down | PostgreSQL |

### Environment variables

| Variable | Default | Overrides |
|---|---|---|
| `DATABASE_URL` | `postgres://yorishiro:yorishiro@localhost:15432/yorishiro` | Target database for any test command |
| `DB_MAX_CONNECTIONS` | *(none)* | Passed to `test-postgres` and `entities` |
| `DB_CONNECT_TIMEOUT` | *(none)* | Passed to `test-postgres` and `entities` |
| `LOCO_ENV` | *(none)* | Passed to `test-postgres` and `entities` (`test_postgres`); or `test_sqlite` for SQLite |

Custom PostgreSQL:

```sh
make test-postgres DATABASE_URL=postgres://user:pass@host:5432/db
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
- `tests/metaschema/` — nesting, projection, validation, type resolution, versioning
- `tests/migration/` — `postgres.rs`, `sqlite.rs`
- `tests/models/` — entity CRUD, search, tenancy, templates, API keys, recall
- `tests/requests/` — auth, schemas, workspaces, entities, search, OAuth, Stripe, marketplace, dashboard, embedding, import, queue, etc.
- `tests/services/` — embedding, rate limiting
- `tests/tasks/` — task commands (reindex_embeddings)
- `tests/workers/` — embedding_sync

There is no `tests/lib.rs` and no `tests/test_helpers.rs`.

## SQLite gate

`tests/mod.rs` exports `require_sqlite_backend()` which returns `true` when `DATABASE_URL` starts with `sqlite://` or `sqlite::memory:`. Call sites:

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
