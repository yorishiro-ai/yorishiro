# Work plan: backend seams without changing search results

Working document for `refactor/iam-and-backend-seams`.
Delete it before the pull request is opened.

## Decisions

- SQLite and PostgreSQL both stay supported. Performance means PostgreSQL for the database and PostgreSQL or Valkey (Redis) for the queue; RabbitMQ is a later Loco provider, not part of this work.
- Search results do not change. The PostgreSQL hybrid search (vector + `pg_trgm` through RRF, `@>` filter) and the SQLite search (sqlite-vec, FTS5 fill-in, no filter) keep their queries and ranking.
- Backend differences live behind one seam each. Models and controllers do not call `is_sqlite` or `is_postgres`.
- Tests: no push, PR or merge without authorisation. Run the suites once at the end.

## Steps

### 1. Search seam
- [ ] `SearchBackend` trait in `src/models/search/` with `PostgresSearch` and `SqliteSearch`, moving today's queries as they are.
- [ ] Shared part only: input validation, width check, availability check, mapping rows to hits.
- [ ] Parity test per backend: a fixed corpus returns a fixed ranking, recorded before the move and asserted after.

### 2. Full-text index seam
- [ ] SQLite FTS5 maintenance (`forget_full_text`, `resync_full_text`, retirement cleanup) sits behind the same seam; entity models call it through the trait.

### 3. Locks
- [ ] One small interface in `src/db.rs` for the transaction lock, the long-held lock (scheduler ownership, reindex, HNSW maintenance).
- [ ] PostgreSQL uses advisory locks; SQLite uses `BEGIN IMMEDIATE` and the lock file.
- [ ] No `is_sqlite` or `is_postgres` outside `src/db.rs`, `ee/db.rs`, the search backends and migrations.

### 4. Queue
- [ ] Keep Loco providers as they are; confirm `QueuePolicy` and worker registration do not depend on the provider kind beyond the existing queue module.

### 5. Tests and docs
- [ ] Narrow `SQLITE_TEST_FILTERS` to the SQLite implementations plus smoke tests.
- [ ] Update `AGENTS.md` and `.agents/rules/testing.md`.
- [ ] `make check-all check-all-ee`, SQLite and PostgreSQL suites for CE and EE.
