# Work plan: embedding-only search and backend seams

Working document for `refactor/iam-and-backend-seams`.
Delete it before the pull request is opened.

## Decisions

- SQLite and PostgreSQL both stay supported. Performance means PostgreSQL for the database and PostgreSQL or Valkey (Redis) for the queue; RabbitMQ is a later Loco provider, not part of this work.
- Both backends use embedding-only search and the same final cosine ranking.
- PostgreSQL keeps pgvector HNSW for performance; SQLite keeps exact sqlite-vec candidate selection.
- Approximate PostgreSQL candidate selection can differ from SQLite on large workspaces.
- Search applies the same JSON containment filter to the bounded candidate set on both backends.
- No comparison against the old hybrid ranking is required.
- Search and locking choose the backend in their infrastructure code.
- Unrelated SQL differences, including RLS and schema fork integrity, stay unchanged.
- Tests: no push, PR or merge without authorisation. Run the suites once at the end.

## Steps

### 1. Search seam
- [x] Separate pgvector and sqlite-vec candidate queries under `src/models/search/`; dispatch by backend without a new trait.
- [x] Share validation, filtering, final ranking, and result limits.
- [x] Verify ranking and filtering on both backends using the existing fixed-corpus tests.

### 2. Remove lexical search
- [x] Remove RRF, FTS5 and `pg_trgm` search, index maintenance hooks, and provisioning dependencies.
- [x] Regenerate SeaORM entities after changing the migrations.

### 3. Locks
- [x] Keep transaction locks in `src/db.rs` and share PostgreSQL session-lock acquisition for scheduler ownership, reindex, and HNSW maintenance.
- [x] Keep SQLite write transactions and scheduler lock files without imposing a session lock on SQLite reindex.

### 4. Queue
- [x] Keep Loco providers as they are. Dispatch uses `AppContext::queue_provider`; worker registration and edition queue policies do not select the transport.

### 5. Tests and docs
- [x] Narrow `SQLITE_TEST_FILTERS` to SQLite-specific tests and smoke tests.
- [x] Update `AGENTS.md` and `.agents/rules/testing.md`.
- [x] `make check-all check-all-ee`, SQLite and PostgreSQL suites for CE and EE.

## Verification

`make entities` and `make check-all check-all-ee` pass.
The SQLite subset passes 36 CE and 73 EE integration tests.
The full PostgreSQL suite passes 371 CE and 600 EE integration tests, with one ignored in each edition.
