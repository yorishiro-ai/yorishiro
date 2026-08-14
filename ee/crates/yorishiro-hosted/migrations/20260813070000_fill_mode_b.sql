-- Fill mode B: inferring a missing field value from a language model.
--
-- This edition owns the feature. The community edition built it first, on the reading that a
-- bring-your-own-key design costs the deployment nothing and could therefore live there; what
-- actually decides an edition is that the server makes an outbound chat completion at all.
-- Community dropped both tables in its v0.44.0.
--
-- `IF NOT EXISTS` on both, because which side created them depends on the vendored pin, and
-- the two migration sets are concatenated vendor-first rather than sorted by version:
--
--   * against vendor v0.43.0, whose consolidated initial still creates them, this is a no-op
--   * against v0.44.0 and later, where the initial no longer does and its own drop has run,
--     this is the create
--
-- So a deployment can lockstep across that boundary without a migration ordering problem, and
-- a fresh database gets the same schema either way.

CREATE TABLE IF NOT EXISTS identity.workspace_llm_keys (
  workspace_id UUID PRIMARY KEY REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  base_url     TEXT NOT NULL,
  model        TEXT NOT NULL,
  api_key      TEXT NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS content.fill_proposals (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  job_id        UUID NOT NULL,
  workspace_id  UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  entity_id     UUID NOT NULL,
  field_name    TEXT NOT NULL,
  proposed      JSONB NOT NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, job_id, entity_id, field_name)
);

CREATE INDEX IF NOT EXISTS fill_proposals_job_idx
  ON content.fill_proposals (workspace_id, job_id);

-- RLS on `fill_proposals` only, matching what the community initial applied. `ENABLE` without
-- `FORCE`: the owner must stay able to reach it, and `yorishiro_app` is not the owner.
-- Re-running is harmless, which matters because the vendored initial may already have done it.
ALTER TABLE content.fill_proposals ENABLE ROW LEVEL SECURITY;

-- Read leniently, as the community initial does: the control-plane pool reaches this table over
-- a connection that names no workspace.
DROP POLICY IF EXISTS workspace_isolation ON content.fill_proposals;
CREATE POLICY workspace_isolation ON content.fill_proposals
  USING (workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid);

-- `content.fill_proposals` only. **`identity.workspace_llm_keys` deliberately gets no GRANT**:
-- it holds a workspace's API key in plaintext, and the repository reaches it through the
-- migration-role pool rather than the request role. Without a grant, a query that arrived on a
-- request connection fails at the permission check -- which is a stronger guarantee than an RLS
-- policy being written correctly, and does not silently weaken if a policy is later edited. The
-- community initial granted `content` wholesale and this table not at all; that asymmetry is
-- the design, not an oversight, so it is reproduced rather than tidied up.
GRANT SELECT, INSERT, UPDATE, DELETE ON content.fill_proposals TO yorishiro_app;
