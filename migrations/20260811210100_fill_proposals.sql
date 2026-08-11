-- Values an LLM proposed for fields an entity is missing, held until someone confirms them.
--
-- Mode B infers rather than computes: unlike `default`-filling, which reads the value out of
-- the schema, this one produces a guess from surrounding text. A guess written straight into
-- `content.entities` becomes indistinguishable from a value a person entered, so it waits here
-- and is applied only when a caller confirms the job.
--
-- Keyed by the same `job_id` `content.entity_snapshots` uses, so confirming a job snapshots and
-- applies under one id and `undo_job` reverses it with the machinery that already exists.
CREATE TABLE content.fill_proposals (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  job_id        UUID NOT NULL,
  workspace_id  UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  -- No foreign key to content.entities, matching entity_snapshots: an entity deleted between
  -- proposal and confirmation should leave the proposal readable rather than vanish. Confirm
  -- skips proposals whose entity is gone.
  entity_id     UUID NOT NULL,
  -- The field the value is for, and what was proposed. One row per field rather than per
  -- entity, so a caller can confirm a job where one field's guesses are good and another's are
  -- not -- rejecting a whole entity because one field read badly would push people to accept
  -- all of it.
  field_name    TEXT NOT NULL,
  proposed      JSONB NOT NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

  UNIQUE (workspace_id, job_id, entity_id, field_name)
);

-- Confirm and the proposals listing both read a whole job.
CREATE INDEX fill_proposals_job_idx
  ON content.fill_proposals (workspace_id, job_id);

ALTER TABLE content.fill_proposals ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.fill_proposals FORCE ROW LEVEL SECURITY;

CREATE POLICY workspace_isolation ON content.fill_proposals
  USING (workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid);

GRANT SELECT, INSERT, DELETE ON content.fill_proposals TO yorishiro_app;
