-- Schemas belong to a workspace, not to a tenant.
--
-- A schema was tenant-scoped, so every workspace under a tenant shared one set of rows: editing
-- a schema reached every workspace at once, and one workspace could not add a field without
-- imposing it on its siblings. Applying a template now produces a copy the workspace owns.
--
-- The foreign keys already supported this. `identity.workspaces.schema_id` names one schema per
-- workspace and `content.entities.schema_id` points at a schema directly; only the uniqueness
-- rule and the RLS policy were tenant-wide. Those are what change here.

-- `tenant_id` stays. It is what lets a query reach every schema in a tenant without joining
-- through `identity.workspaces` -- the marketplace and the export path both want that -- and
-- dropping it would also drop the column `content.entities` has no replacement for.
ALTER TABLE content.schemas
  ADD COLUMN workspace_id UUID REFERENCES identity.workspaces(id) ON DELETE CASCADE;

-- Existing rows: give each schema to the workspace that points at it. A schema no workspace
-- references belongs to the tenant's first workspace rather than being deleted -- it may be an
-- archived version an entity still records in `schema_version`.
UPDATE content.schemas s
   SET workspace_id = w.id
  FROM identity.workspaces w
 WHERE w.schema_id = s.id;

UPDATE content.schemas s
   SET workspace_id = (
        SELECT w.id FROM identity.workspaces w
         WHERE w.tenant_id = s.tenant_id
         ORDER BY w.created_at
         LIMIT 1)
 WHERE s.workspace_id IS NULL;

-- A tenant with no workspace at all cannot own a schema, and nothing can reference one.
DELETE FROM content.schemas WHERE workspace_id IS NULL;

ALTER TABLE content.schemas
  ALTER COLUMN workspace_id SET NOT NULL;

-- Uniqueness moves with the scope: two workspaces may now hold a schema of the same name, and
-- versioning is per workspace rather than per tenant.
ALTER TABLE content.schemas
  DROP CONSTRAINT schemas_tenant_name_version_key;

ALTER TABLE content.schemas
  ADD CONSTRAINT schemas_workspace_name_version_key UNIQUE (workspace_id, name, version);

CREATE INDEX schemas_workspace_id_idx ON content.schemas (workspace_id);

-- RLS follows the same axis as `content.entities` and `content.relations`. Left on
-- `app.current_tenant`, a workspace would keep seeing its siblings' schemas -- the isolation
-- this change exists to provide would not hold.
--
-- `missing_ok` on current_setting: the control-plane pool connects without setting either
-- variable, and `current_setting(name)` raises rather than returning NULL when unset.
DROP POLICY tenant_isolation ON content.schemas;

CREATE POLICY workspace_isolation ON content.schemas
  USING (workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid);
