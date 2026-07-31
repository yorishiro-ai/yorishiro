-- Consolidated initial migration (replaces 11 incremental files).
-- This file produces the exact same schema as running those 11 in sequence on a fresh database.

-- Extensions
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Schemas (namespaces, not "content schemas")
CREATE SCHEMA identity;
CREATE SCHEMA content;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;

--------------------------------------------------------------------------------
-- Identity tables
--------------------------------------------------------------------------------

CREATE TABLE identity.tenants (
  id                  UUID PRIMARY KEY DEFAULT uuidv7(),
  name                TEXT NOT NULL,
  plan                TEXT,
  max_workspaces      INTEGER,
  stripe_customer_id  TEXT UNIQUE,
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE identity.users (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  email         TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  display_name  TEXT,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE identity.tenant_memberships (
  id          UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id   UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  user_id     UUID NOT NULL REFERENCES identity.users(id) ON DELETE CASCADE,
  role        TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, user_id)
);

-- workspaces: schema_id added after content.schemas exists (circular FK)
CREATE TABLE identity.workspaces (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id    UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  max_entities INTEGER,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, name)
);

CREATE TABLE identity.api_keys (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  user_id      UUID REFERENCES identity.users(id) ON DELETE SET NULL,
  key_hash     BYTEA NOT NULL UNIQUE,
  key_prefix   TEXT  NOT NULL,
  scope        TEXT  NOT NULL CHECK (scope IN ('read', 'write', 'schema')),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_used_at TIMESTAMPTZ
);

CREATE TABLE identity.invites (
  id          UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id   UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  email       TEXT NOT NULL,
  role        TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
  token_hash  BYTEA NOT NULL UNIQUE,
  expires_at  TIMESTAMPTZ NOT NULL,
  used_at     TIMESTAMPTZ,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX invites_tenant_id_idx ON identity.invites (tenant_id);

CREATE TABLE identity.templates (
  id          UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id   UUID NOT NULL REFERENCES identity.tenants(id),
  name        TEXT NOT NULL,
  description TEXT,
  definition  JSONB NOT NULL,
  tags        TEXT[] NOT NULL DEFAULT '{}',
  locale      TEXT,
  visibility  TEXT NOT NULL DEFAULT 'tenant' CHECK (visibility IN ('tenant', 'community')),
  author      TEXT,
  fork_of     UUID REFERENCES identity.templates(id),
  created_by  UUID REFERENCES identity.users(id),
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, name)
);

CREATE INDEX templates_tenant_id_idx ON identity.templates(tenant_id);
CREATE INDEX templates_tags_idx ON identity.templates USING gin(tags);

--------------------------------------------------------------------------------
-- Content tables
--------------------------------------------------------------------------------

CREATE TABLE content.schemas (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id    UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  version      INTEGER NOT NULL DEFAULT 1,
  definition   JSONB NOT NULL,
  status       TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT schemas_tenant_name_version_key UNIQUE (tenant_id, name, version)
);

CREATE INDEX schemas_tenant_id_idx ON content.schemas (tenant_id);

-- Now that content.schemas exists, add the circular FK from workspaces
ALTER TABLE identity.workspaces
  ADD COLUMN schema_id UUID REFERENCES content.schemas(id);

CREATE TABLE content.entities (
  id             UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id   UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  schema_id      UUID NOT NULL REFERENCES content.schemas(id),
  schema_version INTEGER NOT NULL,
  entity_type    TEXT NOT NULL,
  data           JSONB NOT NULL,
  embedding      vector(768),
  created_by     UUID REFERENCES identity.users(id) ON DELETE SET NULL,
  updated_by     UUID REFERENCES identity.users(id) ON DELETE SET NULL,
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX entities_workspace_type_idx ON content.entities (workspace_id, entity_type, created_at);
CREATE INDEX entities_data_gin           ON content.entities USING GIN (data jsonb_path_ops);
CREATE INDEX entities_embedding_hnsw     ON content.entities USING hnsw (embedding vector_cosine_ops);
CREATE INDEX entities_data_trgm_idx      ON content.entities USING gin ((data::text) gin_trgm_ops);

-- Remove dimension constraint so operators can use any embedding model.
-- The HNSW index created above is preserved by PostgreSQL across the type change.
ALTER TABLE content.entities ALTER COLUMN embedding TYPE vector;

CREATE TABLE content.relations (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id  UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  source_id     UUID NOT NULL REFERENCES content.entities(id) ON DELETE CASCADE,
  target_id     UUID NOT NULL REFERENCES content.entities(id) ON DELETE CASCADE,
  relation_type TEXT NOT NULL,
  properties    JSONB NOT NULL DEFAULT '{}',
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, source_id, target_id, relation_type)
);

CREATE INDEX relations_source_idx ON content.relations (workspace_id, source_id);
CREATE INDEX relations_target_idx ON content.relations (workspace_id, target_id);

--------------------------------------------------------------------------------
-- Row Level Security
--------------------------------------------------------------------------------

ALTER TABLE identity.tenants             ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.tenant_memberships  ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.workspaces          ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.api_keys            ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.invites             ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.schemas              ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.entities             ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.relations            ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON identity.tenants
  USING (id = current_setting('app.current_tenant')::uuid);

CREATE POLICY tenant_isolation ON identity.tenant_memberships
  USING (tenant_id = current_setting('app.current_tenant')::uuid);

CREATE POLICY tenant_isolation ON identity.workspaces
  USING (tenant_id = current_setting('app.current_tenant')::uuid);

CREATE POLICY workspace_isolation ON identity.api_keys
  USING (workspace_id = current_setting('app.current_workspace')::uuid);

CREATE POLICY tenant_isolation ON identity.invites
  USING (tenant_id = current_setting('app.current_tenant')::uuid);

CREATE POLICY tenant_isolation ON content.schemas
  USING (tenant_id = current_setting('app.current_tenant')::uuid);

CREATE POLICY workspace_isolation ON content.entities
  USING (workspace_id = current_setting('app.current_workspace')::uuid);

CREATE POLICY workspace_isolation ON content.relations
  USING (workspace_id = current_setting('app.current_workspace')::uuid);

--------------------------------------------------------------------------------
-- Role separation + FORCE RLS
--------------------------------------------------------------------------------

DO $$
BEGIN
  CREATE ROLE yorishiro_app NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOLOGIN;
EXCEPTION
  WHEN duplicate_object OR unique_violation THEN
    NULL;
END
$$;

ALTER TABLE identity.tenants             FORCE ROW LEVEL SECURITY;
ALTER TABLE identity.tenant_memberships  FORCE ROW LEVEL SECURITY;
ALTER TABLE identity.workspaces          FORCE ROW LEVEL SECURITY;
ALTER TABLE identity.api_keys            FORCE ROW LEVEL SECURITY;
ALTER TABLE identity.invites             FORCE ROW LEVEL SECURITY;
ALTER TABLE content.schemas              FORCE ROW LEVEL SECURITY;
ALTER TABLE content.entities             FORCE ROW LEVEL SECURITY;
ALTER TABLE content.relations            FORCE ROW LEVEL SECURITY;

GRANT USAGE ON SCHEMA identity TO yorishiro_app;
GRANT USAGE ON SCHEMA content  TO yorishiro_app;

GRANT SELECT ON identity.workspaces TO yorishiro_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON identity.api_keys TO yorishiro_app;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA content TO yorishiro_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA content TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA content GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA content GRANT USAGE, SELECT ON SEQUENCES TO yorishiro_app;

--------------------------------------------------------------------------------
-- API key authentication function (SECURITY DEFINER, bypasses RLS for lookup)
--------------------------------------------------------------------------------

CREATE FUNCTION identity.authenticate_api_key(p_key_hash bytea)
RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, identity
AS $$
  SELECT k.id, k.workspace_id, w.tenant_id, k.scope, k.user_id
  FROM identity.api_keys k
  JOIN identity.workspaces w ON w.id = k.workspace_id
  WHERE k.key_hash = p_key_hash
$$;

REVOKE ALL ON FUNCTION identity.authenticate_api_key(bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION identity.authenticate_api_key(bytea) TO yorishiro_app;
