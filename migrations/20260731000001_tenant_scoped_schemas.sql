-- Move schema ownership from workspace to tenant.
--
-- Before: content.schemas.workspace_id (each schema belongs to a workspace)
-- After:  content.schemas.tenant_id    (each schema belongs to a tenant, shared across workspaces)
--
-- Workspaces gain a schema_id column (1:1) that selects which tenant schema they use.
-- Existing data: each workspace's schemas stay owned by its tenant; the workspace
-- gets linked to its first active schema (if any).

-- Step 1: Add tenant_id to content.schemas, populated from the workspace's tenant.
ALTER TABLE content.schemas ADD COLUMN tenant_id UUID REFERENCES identity.tenants(id) ON DELETE CASCADE;

UPDATE content.schemas s
   SET tenant_id = w.tenant_id
  FROM identity.workspaces w
 WHERE s.workspace_id = w.id;

ALTER TABLE content.schemas ALTER COLUMN tenant_id SET NOT NULL;

-- Step 2: Add schema_id to identity.workspaces (nullable for now, made NOT NULL after backfill).
ALTER TABLE identity.workspaces ADD COLUMN schema_id UUID REFERENCES content.schemas(id);

-- Backfill: link each workspace to its first active schema (by created_at).
UPDATE identity.workspaces w
   SET schema_id = sub.schema_id
  FROM (
    SELECT DISTINCT ON (workspace_id) workspace_id, id AS schema_id
      FROM content.schemas
     WHERE status = 'active'
     ORDER BY workspace_id, created_at
  ) sub
 WHERE w.id = sub.workspace_id;

-- Step 3: Drop old workspace_id from schemas, replace unique constraint.
DROP INDEX IF EXISTS content.schemas_workspace_id_name_version_key;
ALTER TABLE content.schemas DROP CONSTRAINT IF EXISTS schemas_workspace_id_name_version_key;
ALTER TABLE content.schemas DROP COLUMN workspace_id;
ALTER TABLE content.schemas ADD CONSTRAINT schemas_tenant_name_version_key UNIQUE (tenant_id, name, version);

-- Step 4: Create index on tenant_id for schema lookups.
CREATE INDEX schemas_tenant_id_idx ON content.schemas (tenant_id);

-- Step 5: content.schemas' RLS policy (20260712000002_rls.sql) filtered on the
-- now-dropped workspace_id column. Replace it with a tenant_id-scoped policy so
-- schemas are isolated per app.current_tenant, consistent with the new ownership.
DROP POLICY IF EXISTS workspace_isolation ON content.schemas;
CREATE POLICY tenant_isolation ON content.schemas
  USING (tenant_id = current_setting('app.current_tenant')::uuid);
