-- The PostgreSQL image runs this script as its bootstrap superuser when initializing an empty data directory.
-- Install both extensions before the application runs migrations.
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Loco's request tests need CREATEDB to create disposable databases.
\c postgres
ALTER ROLE yorishiro CREATEDB;

-- Disposable test databases inherit extensions from template1, not from POSTGRES_DB.
\c template1
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
