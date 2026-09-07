# SQLite mode

Yorishiro can run on SQLite instead of PostgreSQL. Use this for local evaluation, personal use, or a single-person workspace that does not share data with anyone else.

Do not use SQLite for multi-tenant hosting. It stores all data in a single file with no database-level tenant isolation. PostgreSQL is required when two or more tenants share the same deployment.

## What works on SQLite

- **Entity CRUD.** Create, read, update, and delete content entities.
- **Vector search.** Similarity search using embedded vectors (sqlite-vec).
- **Full-text search.** Text search with character-n-gram fuzzy matching via FTS5 `tokenize='trigram'`.
- **API key authentication.** Create and use API keys.
- **Embedding sync.** Generate and store vectors the same way as PostgreSQL.
- **Snapshots and undo.** Restore entities from a snapshot (requires a prior snapshot created on PostgreSQL).

## What does not work on SQLite

- **Multiple tenants.** SQLite supports exactly one tenant. The tenant cap is hardcoded to 1 and cannot be changed.
- **Content filtering.** The `filter` query parameter (JSONB containment) returns an error on SQLite.
- **Enterprise features requiring PostgreSQL SQL.** Features that depend on `unnest`, `CROSS JOIN LATERAL`, or advisory locks are unavailable.
- **SQLite snapshots.** Creating snapshots is not supported on SQLite. A snapshot created on PostgreSQL can be restored on SQLite.

## Configuration

Yorishiro uses the database URL scheme to detect SQLite:

```sh
export DATABASE_URL='sqlite:///var/lib/yotsunagi/yorishiro.sqlite3?mode=rwc'
cargo run
```

At least 2 connections are required. The server refuses to start if `max_connections` is less than 2.

## Configuration files

A test config exists at `config/test_sqlite.yaml` for manual verification. There is no production SQLite config file; use `production.yaml` and set `DATABASE_URL` to a `sqlite://` URL.

## Testing

SQLite tests run via `make test-sqlite`. They execute with `--test-threads=1` against an in-memory database to avoid concurrency issues. The full test suite (`make test`) runs against PostgreSQL only.

## Authentication

Authentication bypasses the tenant pool and connects directly to the database. There is no row-level security. This is safe when there is only one tenant.
