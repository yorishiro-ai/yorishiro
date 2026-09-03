# SQLite support

Yorishiro runs on SQLite for evaluation and single-tenant personal use.  It is not positioned for multi-tenant hosting, since SQLite has no row-level security to isolate tenants.

## Limitations

### No multi-tenant isolation

SQLite stores all data in a single file with no database-enforced tenant separation.  The application applies filtering, but a single missed filter would be a silent isolation break.  Use PostgreSQL for any deployment hosting more than one tenant.

### Vector search works

Vector similarity search uses the [sqlite-vec](https://github.com/asg017/sqlite-vec) extension, which loads via `sqlite3_auto_extension` at boot.  The `content_entity_embeddings` table stores vectors as raw LE f32 BLOBs (no PgVector equivalent).  KNN search runs as a full cosine-distance scan ordered by `vec_distance_cosine(ee.embedding, $1)`, which is fast at current scale.  The `content_entities` table itself has no `embedding` column — all vector queries join through `content_entity_embeddings`.

### Full-text search uses FTS5

SQLite has no pg_trgm.  The fallback path for entities with no embedding uses the FTS5 virtual table `fts_content_entities`, created by the migration and kept in sync via triggers.  The FTS5 table carries an `entity_id UNINDEXED` column so that joins use the UUID ID (not implicit rowid, which VACUUM may renumber).  Content is stored in the `data` column, not the auto-populated `content` column, so triggers explicitly INSERT `NEW.id` / `OLD.id` rather than `NEW.rowid` / `OLD.rowid`.

### JSONB filtering is not available

SQLite has no `@>` containment operator.  The `filter` query parameter (JSONB containment) returns `BackendUnsupported` on SQLite.

### No advisory locks

SQLite allows only one write transaction at a time, so advisory locks are unnecessary.  `db::lock_for_update` returns `Ok(())` on SQLite.

### Id generation

Columns with `uuidv7()` defaults on PostgreSQL generate their own id on SQLite through `ActiveModelBehavior::before_save`.  Code using the `Entity::insert(...).on_conflict(...)` builder path (which skips `before_save`) sets ids explicitly via `db::sqlite_generated_id`.

### Embedding sync

The `embeddings` table `content_entity_embeddings` exists on both backends.  Embedding generation and sync work the same way on SQLite; the only difference is the storage format (BLOB vs. PgVector).

## Boot-time behavior

On SQLite, the `DbHandle` (PostgreSQL tenant pool) and `Authenticator` seam are not built.  Authentication goes directly against `ctx.db`.  The boot-time `tracing::warn` lists only the enterprise features that use PostgreSQL-only SQL (`unnest`, `CROSS JOIN LATERAL`, advisory locks); vector search works on this backend.

## Configuration

`config/sqlite.yaml` sets `max_connections: 10`.  SQLite requires at least 2 connections: one for the request transaction and one for the independent `last_used_at` update.  Boot fails with a clear error if `max_connections < 2`.

## Testing

Integration tests (`tests/`) run against PostgreSQL only.  SQLite is a manual-verification-only environment (`LOCO_ENV=sqlite`), not wired into the test suite, since `CREATE DATABASE` (used by the test harness) has no SQLite equivalent.
