-- A workspace starts empty and says so.
--
-- Creating an entity before any schema exists already failed, but it failed as a 404 on the
-- schema name -- which reads as "you typed the wrong name", not "nothing is defined here yet".
-- A workspace now carries that state explicitly, so the refusal can say which one it is.
--
-- 'active' for existing rows: they are all past this point, and a workspace that has been in
-- use for weeks must not start refusing writes because a column was added.
ALTER TABLE identity.workspaces
  ADD COLUMN status TEXT NOT NULL DEFAULT 'active'
  CHECK (status IN ('schema_pending', 'active'));

-- New workspaces start pending. Existing ones keep the 'active' backfilled above -- the default
-- changes only what is inserted from here on.
ALTER TABLE identity.workspaces
  ALTER COLUMN status SET DEFAULT 'schema_pending';

-- A workspace that already owns a schema is active whatever the backfill said. This matters for
-- rows created between the two statements above in a concurrent deploy, and it is the same rule
-- the application applies from now on.
UPDATE identity.workspaces w
   SET status = 'active'
 WHERE EXISTS (SELECT 1 FROM content.schemas s WHERE s.workspace_id = w.id);
