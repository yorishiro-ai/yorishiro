-- Run automatically by the postgres image's docker-entrypoint-initdb.d on first init (empty
-- data directory only, never on a restart against an existing volume).
-- pgvector isn't part of contrib, so a non-superuser can't CREATE EXTENSION it later; doing it
-- here, once, as the image's bootstrap superuser, is simpler than teaching the migration crate
-- to run as one. pg_trgm ships with contrib and could be created lazily, but keeping both here
-- keeps this file the single place a fresh volume's prerequisites are documented.
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Grant CREATEDB to yorishiro so loco's request-test harness (request_with_create_db)
-- can CREATE DATABASE throwaway test databases. Without this, every request test fails
-- with PoolTimedOut during boot.
\c postgres
ALTER ROLE yorishiro CREATEDB;

-- Also into template1: Loco's request-test harness (loco_rs::testing::request_with_create_db)
-- creates each throwaway test database with CREATE DATABASE, which copies template1, not the
-- POSTGRES_DB this script otherwise runs against. Without this, converge fails on
-- content_entities.embedding inside every request test, and the resulting boot-error panics
-- during loco_rs's own cleanup_db rather than surfacing as the actual error.
\c template1
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
