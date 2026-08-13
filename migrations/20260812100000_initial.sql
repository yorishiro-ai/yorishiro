-- The schema, stated once.
--
-- This replaces fifteen incremental files. It is not a replay of their history: every column
-- lands in its final shape, so the columns those files added by `ALTER` are simply declared,
-- the constraints they replaced never exist, and the data backfills they carried are gone —
-- there are no users yet, so there is nothing to migrate. Running this on an empty database
-- produces what running those fifteen produced, minus the marketplace tables (see below).
--
-- Two things that were bugs are folded in rather than appended, because appending them is how
-- they came to be missing in the first place:
--
--   * `GRANT yorishiro_app TO CURRENT_USER`, without which a non-superuser login role cannot
--     `SET ROLE yorishiro_app` at all. Every deployment here has run as a superuser, where the
--     grant is implicit, so nothing ever noticed; a CI database whose role is not a superuser
--     cannot start.
--   * `GRANT UPDATE (status, schema_id) ON identity.workspaces`, without which `create_schema`
--     answered 500 on every path (#129).
--
-- `identity.template_versions` / `template_reviews` are NOT here. Template publishing and
-- reviews are not part of Yorishiro; nothing in this repository reads those tables.

CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE SCHEMA identity;
CREATE SCHEMA content;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;

--------------------------------------------------------------------------------
-- The application role
--------------------------------------------------------------------------------
--
-- Created before anything it needs rights on, so the grants below can be written beside the
-- objects they apply to.

DO $$
BEGIN
  CREATE ROLE yorishiro_app NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOLOGIN;
EXCEPTION
  WHEN duplicate_object OR unique_violation THEN
    NULL;
END
$$;

-- The connecting role must be a member of `yorishiro_app` to `SET ROLE` to it. A superuser may
-- do so without membership, which is why this was never needed here and is needed everywhere
-- else: PostgreSQL 16+ refuses `SET ROLE` for a non-superuser creator, and refuses
-- `CREATE ROLE ... ADMIN <grantor>` as a shortcut. `CURRENT_USER` is whoever runs the
-- migration, so this is a no-op for a superuser and the only thing that works otherwise.
GRANT yorishiro_app TO CURRENT_USER;

--------------------------------------------------------------------------------
-- Identity
--------------------------------------------------------------------------------

CREATE TABLE identity.tenants (
  id                  UUID PRIMARY KEY DEFAULT uuidv7(),
  name                TEXT NOT NULL,
  max_workspaces      INTEGER,
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

-- `schema_id` is added after `content.schemas` exists -- the reference is circular, since a
-- schema also names its workspace.
CREATE TABLE identity.workspaces (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id    UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  max_entities INTEGER,
  -- A workspace exists before its schema does: `admin create-workspace` leaves it pending, and
  -- creating the schema marks it active.
  status       TEXT NOT NULL DEFAULT 'schema_pending'
                 CHECK (status IN ('schema_pending', 'active')),
  -- The model a workspace's vectors were produced by, and their width. NULL means the
  -- deployment default -- recorded so a workspace whose model changed can be told from one
  -- provisioned under a different one.
  embedding_model      TEXT,
  embedding_dimensions INTEGER CHECK (embedding_dimensions IS NULL OR embedding_dimensions > 0),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, name)
);

CREATE TABLE identity.api_keys (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  user_id      UUID REFERENCES identity.users(id) ON DELETE SET NULL,
  key_hash     BYTEA NOT NULL UNIQUE,
  key_prefix   TEXT  NOT NULL,
  -- `migration` ranks above `schema`: registering a schema adds a version nothing has been
  -- written against yet, while a batch migration rewrites stored rows.
  scope        TEXT  NOT NULL CHECK (scope IN ('read', 'write', 'schema', 'migration')),
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
  -- `ON DELETE SET NULL`: deleting a template others were forked from must leave the forks
  -- usable, losing only the pointer back.
  fork_of     UUID REFERENCES identity.templates(id) ON DELETE SET NULL,
  created_by  UUID REFERENCES identity.users(id),
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, name)
);

CREATE INDEX templates_tenant_id_idx ON identity.templates(tenant_id);
CREATE INDEX templates_tags_idx ON identity.templates USING gin(tags);

-- One row, enforced by the primary key. `read_only` sheds writes; `full_lock` sheds everything
-- but the health probes.
CREATE TABLE identity.maintenance (
  id          BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
  mode        TEXT NOT NULL DEFAULT 'off'
              CHECK (mode IN ('off', 'read_only', 'full_lock')),
  retry_after INTEGER NOT NULL DEFAULT 300 CHECK (retry_after > 0),
  reason      TEXT,
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO identity.maintenance (id) VALUES (TRUE);

-- The LLM-backed fill path. This table and `content.fill_proposals` are on their way out --
-- Yorishiro makes no outbound model calls -- but the code still queries them at this version,
-- and a migration that omitted a table the code reads would refuse to boot. A later migration
-- drops both.
CREATE TABLE identity.workspace_llm_keys (
  workspace_id UUID PRIMARY KEY REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  base_url     TEXT NOT NULL,
  model        TEXT NOT NULL,
  api_key      TEXT NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

--------------------------------------------------------------------------------
-- Content
--------------------------------------------------------------------------------

CREATE TABLE content.schemas (
  id           UUID PRIMARY KEY DEFAULT uuidv7(),
  tenant_id    UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  -- Schemas are scoped to a workspace, not a tenant: each workspace holds its own copy of a
  -- template, and editing one must not reach its siblings. `tenant_id` stays for the
  -- cross-tenant reads (community-visible templates, export).
  workspace_id UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  version      INTEGER NOT NULL DEFAULT 1,
  definition   JSONB NOT NULL,
  status       TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
  -- Where this schema came from, and whether it still follows it. A hand-written schema is
  -- `detached` and has never been linked -- told apart from an orphan by `origin_template_id`
  -- having never been set. `origin_snapshot` is the definition as copied, which is what a
  -- three-way comparison needs as its base.
  origin_template_id UUID REFERENCES identity.templates(id) ON DELETE SET NULL,
  origin_status      TEXT NOT NULL DEFAULT 'detached'
                       CHECK (origin_status IN ('linked', 'detached')),
  origin_snapshot    JSONB,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT schemas_workspace_name_version_key UNIQUE (workspace_id, name, version)
);

CREATE INDEX schemas_tenant_id_idx    ON content.schemas (tenant_id);
CREATE INDEX schemas_workspace_id_idx ON content.schemas (workspace_id);
CREATE INDEX schemas_origin_template_idx
  ON content.schemas (origin_template_id)
  WHERE origin_template_id IS NOT NULL;

-- The circular half: a workspace names its schema, and a schema names its workspace.
ALTER TABLE identity.workspaces
  ADD COLUMN schema_id UUID REFERENCES content.schemas(id);

-- Deleting a template must not destroy the copies made from it, and must stop them claiming to
-- follow something that is gone. A trigger rather than application code, so a delete arriving
-- from the admin CLI or a migration is covered too.
CREATE FUNCTION content.detach_orphaned_schema_origin() RETURNS TRIGGER AS $$
BEGIN
  UPDATE content.schemas
     SET origin_status = 'detached'
   WHERE origin_template_id = OLD.id
     AND origin_status = 'linked';
  RETURN OLD;
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

CREATE TRIGGER templates_detach_schema_origins
  BEFORE DELETE ON identity.templates
  FOR EACH ROW EXECUTE FUNCTION content.detach_orphaned_schema_origin();

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

-- The column is declared with a width only so the HNSW index above can be built; dropping the
-- constraint afterwards lets an operator use any model. PostgreSQL keeps the index across the
-- type change, and one index suffices because a workspace's vectors are all one width.
ALTER TABLE content.entities ALTER COLUMN embedding TYPE vector;

CREATE TABLE content.relations (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  workspace_id  UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  source_id     UUID NOT NULL REFERENCES content.entities(id) ON DELETE CASCADE,
  target_id     UUID NOT NULL REFERENCES content.entities(id) ON DELETE CASCADE,
  relation_type TEXT NOT NULL,
  properties    JSONB NOT NULL DEFAULT '{}',
  status        TEXT NOT NULL DEFAULT 'active'
                  CHECK (status IN ('active', 'deprecated', 'archived')),
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, source_id, target_id, relation_type)
);

-- `status` is in both indexes because every traversal filters on it.
CREATE INDEX relations_source_idx ON content.relations (workspace_id, source_id, status);
CREATE INDEX relations_target_idx ON content.relations (workspace_id, target_id, status);

-- What an entity looked like before a batch migration touched it, so the migration can be
-- undone. Keyed by job, and swept by age.
CREATE TABLE content.entity_snapshots (
  id             UUID PRIMARY KEY DEFAULT uuidv7(),
  job_id         UUID NOT NULL,
  workspace_id   UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  entity_id      UUID NOT NULL,
  schema_id      UUID NOT NULL,
  schema_version INTEGER NOT NULL,
  data           JSONB NOT NULL,
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX entity_snapshots_job_idx
  ON content.entity_snapshots (workspace_id, job_id);
CREATE INDEX entity_snapshots_entity_idx
  ON content.entity_snapshots (workspace_id, entity_id, created_at DESC);

-- Fill mode B, as above: goes with `identity.workspace_llm_keys` when the frozen move lands.
CREATE TABLE content.fill_proposals (
  id            UUID PRIMARY KEY DEFAULT uuidv7(),
  job_id        UUID NOT NULL,
  workspace_id  UUID NOT NULL REFERENCES identity.workspaces(id) ON DELETE CASCADE,
  entity_id     UUID NOT NULL,
  field_name    TEXT NOT NULL,
  proposed      JSONB NOT NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, job_id, entity_id, field_name)
);

CREATE INDEX fill_proposals_job_idx
  ON content.fill_proposals (workspace_id, job_id);

--------------------------------------------------------------------------------
-- Row-level security
--------------------------------------------------------------------------------
--
-- ENABLE, deliberately without FORCE. `ENABLE` alone already constrains `yorishiro_app`, which
-- is not the owner, so tenant isolation does not depend on FORCE at all -- verified in both
-- directions: scoped to one tenant the app role sees only that tenant's rows, and with no
-- tenant named it sees nothing (the strict `current_setting` below raises rather than
-- filtering, so the failure is loud).
--
-- FORCE would additionally subject the *owner* to the policies, and the owner must not be:
--
--   * `identity.authenticate_api_key` runs as the owner precisely because no workspace is known
--     yet -- there is nothing to scope to until the key resolves one. Under FORCE it evaluates
--     an unset `app.current_workspace` and raises `unrecognized configuration parameter`, so no
--     request can authenticate at all.
--   * The admin CLI creates tenants, workspaces, memberships and invites as the owner, before
--     the ids those policies compare against exist.
--
-- Both are broken only when the owner is *not* a superuser, since a superuser bypasses RLS
-- whatever FORCE says -- so FORCE has never taken effect in any deployment here, and the first
-- environment where it would (a non-superuser CI database) is the one it stops working.
--
-- Making the policies lenient instead does not fix it: with the GUCs unset a lenient policy
-- matches no rows, so `authenticate_api_key` returns nothing for a *valid* key and every
-- request fails authentication silently. Measured, not reasoned.
--
-- `identity.templates` is deliberately absent. Template queries run as the owner through the
-- repository layer, which scopes by tenant in the query, because a policy would have to read
-- the table the app role holds no grant on -- and a policy the role cannot evaluate fails the
-- query rather than filtering it.

ALTER TABLE identity.tenants             ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.tenant_memberships  ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.workspaces          ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.api_keys            ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity.invites             ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.schemas              ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.entities             ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.relations            ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.entity_snapshots     ENABLE ROW LEVEL SECURITY;
ALTER TABLE content.fill_proposals       ENABLE ROW LEVEL SECURITY;

-- No `FORCE ROW LEVEL SECURITY` here, for the reasons above. The owner must stay able to
-- authenticate a key and to provision tenants; `ENABLE` alone is what constrains the app role.

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

-- Two forms follow, and the difference is deliberate.
--
-- `content.schemas`, `entity_snapshots` and `fill_proposals` read the setting with `true`
-- (missing is NULL rather than an error) and fold the empty string to NULL, so a connection
-- that has not named a workspace matches nothing instead of failing. They need it because the
-- control-plane pool reaches them over a connection that sets neither variable.
--
-- Everything else uses the strict form on purpose. `yorishiro_app` sets both GUCs on every
-- connection, so reaching one of those tables without a workspace is a bug -- and raising
-- surfaces it, where matching zero rows would look like an empty workspace. Unifying on the
-- lenient form was measured and is worse: it would make an unset GUC return no rows for a
-- *valid* API key, so every request would fail authentication with nothing logged anywhere.
CREATE POLICY workspace_isolation ON content.schemas
  USING (workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid);

CREATE POLICY workspace_isolation ON content.entities
  USING (workspace_id = current_setting('app.current_workspace')::uuid);

CREATE POLICY workspace_isolation ON content.relations
  USING (workspace_id = current_setting('app.current_workspace')::uuid);

CREATE POLICY workspace_isolation ON content.entity_snapshots
  USING (workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid);

CREATE POLICY workspace_isolation ON content.fill_proposals
  USING (workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid);

--------------------------------------------------------------------------------
-- Grants
--------------------------------------------------------------------------------

GRANT USAGE ON SCHEMA identity TO yorishiro_app;
GRANT USAGE ON SCHEMA content  TO yorishiro_app;

GRANT SELECT ON identity.workspaces TO yorishiro_app;

-- Column-level, because these two are the whole of what a request writes here. `max_entities`,
-- `name` and the embedding stamp are provisioning decisions, and a request that could rewrite
-- its own quota is a different system.
GRANT UPDATE (status, schema_id) ON identity.workspaces TO yorishiro_app;

GRANT SELECT, INSERT, UPDATE, DELETE ON identity.api_keys TO yorishiro_app;
GRANT SELECT ON identity.maintenance TO yorishiro_app;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA content TO yorishiro_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA content TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA content GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO yorishiro_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA content GRANT USAGE, SELECT ON SEQUENCES TO yorishiro_app;

--------------------------------------------------------------------------------
-- API key authentication
--------------------------------------------------------------------------------
--
-- `SECURITY DEFINER` so the lookup can read rows RLS would hide: the caller has not been
-- identified yet, so there is no workspace to scope to.
--
-- One argument, and the overload set is open: a downstream deployment may add a two-argument
-- form for keys that name their workspace per request. This one resolves a key bound to a
-- single workspace, and correctly returns nothing for a key that carries none.

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
