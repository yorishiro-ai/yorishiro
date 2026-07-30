-- Remove the fixed 768-dimension constraint on the embedding column so operators
-- can use any embedding model (e.g. text-embedding-3-small at 1536, nomic-embed-text
-- at 768, Japanese models at 1024). The HNSW index is rebuilt automatically by
-- PostgreSQL when the column type changes.
--
-- All vectors in a given deployment must still share the same dimensionality
-- (enforced at application startup via YSR_EMBEDDING_DIMENSIONS), but that
-- dimension is no longer hardcoded to 768.
ALTER TABLE content.entities ALTER COLUMN embedding TYPE vector;
