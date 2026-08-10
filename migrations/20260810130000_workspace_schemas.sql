-- Workspace-scoped schema forks.
--
-- A schema belongs to a tenant and every workspace under it shares the same rows, so editing a
-- schema reaches every workspace at once -- one workspace cannot add a field without imposing
-- it on the others. A workspace may now fork its tenant's schema: the fork is a copy it owns
-- and can edit, and a later edit to the tenant's own schema does not reach it.
--
-- content.schemas is left exactly as it is. Every existing query, RLS policy and foreign key
-- against it keeps working, and a workspace that never forks behaves as it always did.

CREATE TABLE content.workspace_schemas (
  id             UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id   UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  source_id      UUID NOT NULL REFERENCES content.schemas(id),
  -- The source's version at fork time. Comparing it against the source's current version is
  -- what makes "the tenant's schema has moved on" answerable without diffing definitions.
  source_version INTEGER NOT NULL,
  definition     JSONB NOT NULL,
  -- Set once the fork's definition stops matching what was copied. Follow-the-master can then
  -- overwrite an untouched fork without asking, and must ask before discarding local edits.
  customized     BOOLEAN NOT NULL DEFAULT false,
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  -- One fork per workspace. A workspace has a single schema, so a second row would leave
  -- "which schema is this workspace's" ambiguous.
  UNIQUE (workspace_id)
);

CREATE INDEX workspace_schemas_source_idx ON content.workspace_schemas (source_id, source_version);

ALTER TABLE content.workspace_schemas ENABLE ROW LEVEL SECURITY;

-- Same shape as the policies on entities/relations: a session may only see its own workspace.
CREATE POLICY workspace_schemas_isolation ON content.workspace_schemas
  USING (workspace_id = current_setting('app.current_workspace')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE ON content.workspace_schemas TO yorishiro_app;

-- Entities keep pointing at content.schemas. A fork copies its source's definition, so the
-- version an entity was validated against still exists there and `schema_version` still means
-- what it meant. Repointing entities at the fork would rewrite every existing row's schema_id
-- for a feature most workspaces never use.
COMMENT ON TABLE content.workspace_schemas IS
  'A workspace''s own copy of its tenant''s schema. Absent unless the workspace has forked.';
