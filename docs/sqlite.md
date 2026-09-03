# SQLite mode

Yorishiro can run on SQLite instead of PostgreSQL.  Use this mode to try Yorishiro out on your own machine, or for a single-person workspace that does not share data with anyone else.

Do not use SQLite for multi-tenant hosting.  It stores all data in a single file with no database-level tenant isolation.  PostgreSQL is required when two or more tenants share the same deployment.

## What works on SQLite

- **Entity CRUD.**  Create, read, update, and delete content entities.
- **Entity search.**  Text search across all entities.
- **Vector search.**  Similarity search using embedded vectors.
- **Full-text search.**  Text search for entities that have no embedding.
- **API key authentication.**  Create and use API keys.
- **Embedding sync.**  Generate and store vectors the same way as PostgreSQL.
- **Snapshots and undo.**  Restore entities from a snapshot (requires a prior snapshot created on PostgreSQL).

## What does not work on SQLite

- **Multiple tenants.**  SQLite supports exactly one tenant.  The tenant cap is hardcoded to 1 and cannot be changed.
- **JSONB filtering.**  The `filter` query parameter (JSONB containment) is unavailable and returns an error.
- **Enterprise features requiring PostgreSQL SQL.**  Features that depend on `unnest`, `CROSS JOIN LATERAL`, or advisory locks are unavailable.
- **SQLite snapshots.**  The snapshot feature writes using PostgreSQL-only SQL.  A snapshot created on PostgreSQL can be restored on SQLite, but creating one on SQLite is not supported.

## Configuration

Copy the example SQLite config and set the environment to use it:

```sh
cp config/sqlite.yaml.example config/sqlite.yaml
export LOCO_ENV=sqlite
```

`config/sqlite.yaml` sets `max_connections: 10`.  At least 2 connections are required.  The server will refuse to start if `max_connections` is less than 2.

## Trying it out

Start the server as normal.  The boot log will note that enterprise features depending on PostgreSQL-specific SQL are unavailable, and that vector search works on this backend.

Authentication bypasses the tenant pool and connects directly to the database.  This is simpler but also means there is no row-level security.  The application trusts that the single tenant will not tamper with data belonging to others — which is a non-issue when there is only one tenant.

## Testing

The test suite runs against PostgreSQL only.  SQLite is a manual-verification environment.  To verify changes against SQLite:

```sh
export LOCO_ENV=sqlite
cargo run
```
