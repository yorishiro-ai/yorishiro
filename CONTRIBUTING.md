# Contributing

**English** | [日本語](docs/ja/contributing.md)

## Code layout

| Location | Holds |
|---|---|
| `src/models/` | Record shapes, input DTOs, and queries |
| `src/controllers/` | Request handlers |
| `src/services/` | Auth, embedding, MCP |
| `migration/src/` | Schema migrations |
| `src/tasks/` | Admin and one-off commands |
| `ee/` | Enterprise edition (mirrors `models/`, `controllers/`, `services/`) |

- `src/models/_entities/` is auto-generated. Never edit by hand.
- Business logic goes in `src/models/<table>.rs` next to the generated entity.
- Raw SQL is only where SeaORM cannot express it (JSONB containment, pgvector, advisory locks).
- `src/db.rs` handles connections, not tables.

### Database pools

Database connection pools are unrelated to worker pools and the queue.
On PostgreSQL, `AppContext.db` is Loco's SeaORM pool using the migration role, `TenantDb` is the separate RLS pool that sets `ROLE yorishiro_app`, and `DbHandle.identity` is a second migration-role raw sqlx pool.
The Loco queue provider owns another independent pool.
SQLite has no `DbHandle`; it uses `AppContext.db` plus the queue provider's separate SQLite pool.

Production defaults `database.max_connections` to `100` on either backend unless configuration overrides it.
The minimum default is `1`.
CI overrides PostgreSQL `DB_MAX_CONNECTIONS` to `100`, and the SQLite test configuration sets its application pool maximum to `10`.
These limits apply per independent pool, so aggregate database connections are the sum of the pools a process creates.
Async waits release the task to do other work, but one physical connection still executes one SQL operation at a time.

## Adding a table

1. Write a migration under `migration/src/`.
2. Run `make entities` to regenerate `_entities/`.
3. Add a module under `src/models/`.

## Tests

Run the full suite with Rust's default parallel test execution.

PostgreSQL:

```console
$ DATABASE_URL=postgres://yorishiro:yorishiro@localhost:15432/yorishiro \
    DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres \
    cargo test --locked --workspace
```

SQLite:

```console
$ DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' \
    LOCO_ENV=test_sqlite cargo test --locked --workspace
```

Request tests must call `close_app_pools` before returning. See `tests/requests/mod.rs` for the pattern.

Metaschema validation includes property-based tests for arbitrary schema-shaped definitions and rejection of empty `entity_types`.

## Before you push

```console
$ cargo check --locked --workspace
$ cargo clippy --locked --workspace --tests -- -D warnings
$ cargo fmt --all -- --check
```

Tests need PostgreSQL with `vector` and `pg_trgm` in `template1`, and a non-superuser role.

## Git workflow

Branch from `develop`. Merge commits preferred. Every PR updates English + Japanese docs.

## Rules

Rules for code style live at `.claude/rules/`.
