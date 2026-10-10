# SQLite mode

Yorishiro can run on SQLite instead of PostgreSQL. Use this for local evaluation, personal use, or a single-person workspace that does not share data with anyone else.

Do not use SQLite for multi-tenant hosting. It stores all data in a single file with no database-level tenant isolation. PostgreSQL is required when two or more tenants share the same deployment.

## What works on SQLite

- **Entity CRUD.** Create, read, update, and delete content entities.
- **Vector search.** Similarity search using embedded vectors (sqlite-vec).
- **Full-text search.** Text search with character-n-gram fuzzy matching via FTS5 `tokenize='trigram'`.
- **API key authentication.** Create and use API keys.
- **Embedding sync.** Generate and store vectors the same way as PostgreSQL.
- **Template library and schema origin.** Templates (their tags are stored as a JSON array on both backends), creating a schema from a library template, listing upstream changes, and merging upstream template changes.
- **Recall.** `recall_context` traverses relations hop by hop; both backends batch the neighbor lookup at each hop, with a separate limit per pivot.
- **Column preferences and inference proposals** (enterprise).
- **Snapshots and undo.** Restore entities from a snapshot (requires a prior snapshot created on PostgreSQL).

## What does not work on SQLite

- **Multiple tenants.** SQLite supports exactly one tenant. The tenant cap is hardcoded to 1 and cannot be changed.
- **Content filtering.** The `filter` query parameter (JSONB containment) returns an error on SQLite.
- **The marketplace** (enterprise). It lists and publishes across tenants, so its endpoints answer `501` with the `backend_unsupported` code on SQLite.
- **Features that depend on advisory locks.** SQLite has one writer and no named locks, so the lock is a no-op there.
  Infer-fill still runs one job per workspace at a time on SQLite: a unique index on running jobs refuses a second claim, and the worker puts that job back on the queue.
- **SQLite snapshots.** Creating snapshots is not supported on SQLite. A snapshot created on PostgreSQL can be restored on SQLite.

## Configuration
The PostgreSQL template-tags upgrade rewrites the small template table under an exclusive lock and removes the unused tags GIN index.
Rollback restores the original index and converts non-array JSON tags to empty arrays.

All `sqlite:` URL forms, including `sqlite:/path` and `sqlite::memory:`, select SQLite defaults:

```sh
export DATABASE_URL='sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc'
cargo run
```

SQLite background jobs use a separate file, `sqlite:///var/lib/yorishiro/yorishiro_queue.sqlite3?mode=rwc`, by default.
The application database and queue must not share a file because their independent connection pools can hold SQLite locks against each other while a worker dequeues a job.
If an existing installation points `QUEUE_URL` at the application file, move the queue database to the separate path before starting the updated version.
The queue tables are recreated there as needed, so jobs still pending in the old shared file should be drained or migrated by the operator first.
PostgreSQL and Redis queue configurations are unchanged.

At least 2 connections are required. The server refuses to start if `max_connections` is less than 2.

## Configuration files

There is no separate SQLite production config file; use `config/production.yaml` and set `DATABASE_URL` to a `sqlite://` URL.
Set `QUEUE_URL` to another SQLite file when using a custom application path.

## Authentication

Authentication bypasses the tenant pool and connects directly to the database. There is no row-level security. This is safe when there is only one tenant.
