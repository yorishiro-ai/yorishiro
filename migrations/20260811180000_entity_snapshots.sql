-- Before-images of entity data.
--
-- Written before something overwrites an entity, so the previous state can be restored. Two
-- callers want this and neither has it today: a batch migration filling in fields across a
-- workspace, and an ordinary update, which is last-write-wins with nothing kept.
--
-- Grouped by `job_id` so a batch is undone as a batch. A single update gets its own id and is
-- undone alone; nothing distinguishes the two but how many rows share the id.
CREATE TABLE content.entity_snapshots (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  job_id        UUID NOT NULL,
  workspace_id  UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  -- No foreign key to content.entities: a snapshot of an entity that was later deleted is
  -- still the record of what it held, and a cascade would erase exactly the history someone
  -- would go looking for.
  entity_id     UUID NOT NULL,
  schema_id     UUID NOT NULL,
  schema_version INTEGER NOT NULL,
  data          JSONB NOT NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Undo reads a whole job; the workspace column keeps that read inside RLS.
CREATE INDEX entity_snapshots_job_idx
  ON content.entity_snapshots (workspace_id, job_id);

-- And the per-entity history, for reading what one entity held before.
CREATE INDEX entity_snapshots_entity_idx
  ON content.entity_snapshots (workspace_id, entity_id, created_at DESC);

ALTER TABLE content.entity_snapshots ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.entity_snapshots FORCE ROW LEVEL SECURITY;

CREATE POLICY workspace_isolation ON content.entity_snapshots
  USING (workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid);

GRANT SELECT, INSERT, DELETE ON content.entity_snapshots TO yorishiro_app;
