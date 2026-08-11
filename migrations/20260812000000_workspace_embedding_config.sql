-- Which embedding model a workspace's vectors were produced by (FR-1-10).
--
-- The column is dimensionless (`vector`) so a deployment can run any model, and inserts of any
-- dimension succeed. What does not survive is mixing them: a search across two dimension classes
-- fails with `different vector dimensions 384 and 1024` -- at query time, on a whole workspace,
-- far from the write that caused it. Stamping the workspace with what it was created under is
-- what lets that write be refused where it happens.
--
-- Stamped at creation and not edited afterwards. Changing which model a workspace uses means
-- re-embedding every entity in it, which is `admin resync-embeddings` -- an operation someone
-- runs, not a side effect of updating a row.
ALTER TABLE identity.workspaces
  ADD COLUMN embedding_model      TEXT,
  ADD COLUMN embedding_dimensions INTEGER
    CHECK (embedding_dimensions IS NULL OR embedding_dimensions > 0);

-- Existing workspaces stay NULL, which reads as "whatever this deployment is configured for" --
-- exactly what they have always meant. Backfilling today's `YSR_EMBEDDING_DIMENSIONS` would
-- assert a fact about vectors written before that setting had its current value, and a wrong
-- stamp is worse than none: it would reject writes that match what is actually stored.
COMMENT ON COLUMN identity.workspaces.embedding_model IS
  'Model the workspace''s vectors were produced by. NULL means the deployment default.';
COMMENT ON COLUMN identity.workspaces.embedding_dimensions IS
  'Dimension count of those vectors. NULL means the deployment default.';
