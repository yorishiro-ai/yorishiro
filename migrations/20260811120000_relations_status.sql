-- A relation gains a lifecycle state.
--
-- Until now a relation either existed or was deleted, so retiring one destroyed the record that
-- it had ever been there. Graph traversal is the reason this matters: a relation that no longer
-- holds should stop being walked without the history of it being erased.
--
-- 'active' is the default, so every existing row keeps the meaning it already had and every
-- caller that does not mention status keeps working.
ALTER TABLE content.relations
  ADD COLUMN status TEXT NOT NULL DEFAULT 'active'
  CHECK (status IN ('active', 'deprecated', 'archived'));

-- Traversal filters on status within a workspace, which is the access pattern the existing
-- source/target indexes serve. Extending those two rather than adding a third keeps the index
-- count where it was.
--
-- Rebuilt in place rather than CONCURRENTLY: the ADD COLUMN above already takes ACCESS
-- EXCLUSIVE on the table, so building these online would not make the migration online. Doing
-- it properly means splitting this into several non-transactional files, which is worth it once
-- the table is large enough to notice -- a judgement this schema has not had to make yet, and
-- no migration here has needed CONCURRENTLY so far.
DROP INDEX content.relations_source_idx;
DROP INDEX content.relations_target_idx;

CREATE INDEX relations_source_idx ON content.relations (workspace_id, source_id, status);
CREATE INDEX relations_target_idx ON content.relations (workspace_id, target_id, status);
