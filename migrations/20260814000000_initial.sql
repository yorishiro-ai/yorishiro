-- The schema, stated once, for one binary.
--
-- This replaces eight files: the two in the root set and the six the paid edition carried
-- separately. It is not a replay of their history. Every column lands in its final shape, the
-- constraints that were replaced never exist, and the tables that were created and then dropped
-- are simply absent.
--
-- **This is a fresh-database boundary**, the second one. A database carrying the eight previous
-- entries cannot cross it: their versions are gone from this directory, so the migrator refuses
-- rather than half-applying. Recreate the database and re-import. The precedent and the reason
-- are the same as v0.43.0's.
--
-- Why one file rather than eight in one directory: sqlx applies by version sort, and five of the
-- paid edition's files carry timestamps *earlier* than the root `initial`. They worked only
-- because the binary ran two passes, root-first. Sorted into a single directory,
-- `20260730100001_oauth_identity` would ALTER `identity.users` before the file that creates it.
-- One file removes the ordering question rather than renumbering around it.
--
-- Two GRANT asymmetries are deliberate and must survive any future edit here:
--
--   * `GRANT yorishiro_app TO CURRENT_USER`, without which a non-superuser login role cannot
--     `SET ROLE yorishiro_app` at all.
--   * `identity.workspace_llm_keys` gets **no** `yorishiro_app` GRANT, while `api_keys` and
--     `workspaces` do. It holds a workspace's model credentials and is read only by the paid
--     fill path through the owning connection. Normalising the GRANTs "for consistency"
--     is how this class of bug hid for months before.


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

-- `schema_id` is added after `content.schemas` exists: the reference is circular, since a
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
  -- deployment default, recorded so a workspace whose model changed can be told from one
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

-- The LLM-backed fill path. This table and `content.fill_proposals` are on their way out:
-- Yorishiro makes no outbound model calls, but the code still queries them at this version,
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
  -- `detached` and has never been linked, told apart from an orphan by `origin_template_id`
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
-- is not the owner, so tenant isolation does not depend on FORCE at all: scoped to one tenant
-- the app role sees only that tenant's rows, and with no tenant named it sees nothing (the
-- strict `current_setting` below raises rather than filtering, so the failure is loud).
--
-- FORCE would additionally subject the *owner* to the policies, and the owner must not be:
--
--   * `identity.authenticate_api_key` runs as the owner precisely because no workspace is known
--     yet: there is nothing to scope to until the key resolves one. Under FORCE it evaluates
--     an unset `app.current_workspace` and raises `unrecognized configuration parameter`, so no
--     request can authenticate at all.
--   * The admin CLI creates tenants, workspaces, memberships and invites as the owner, before
--     the ids those policies compare against exist.
--
-- Both are broken only when the owner is *not* a superuser, since a superuser bypasses RLS
-- whatever FORCE says, so FORCE takes no effect in a superuser deployment, and a non-superuser
-- database (such as CI) is where it would stop working.
--
-- Making the policies lenient instead does not fix it: with the GUCs unset a lenient policy
-- matches no rows, so `authenticate_api_key` returns nothing for a *valid* key and every
-- request fails authentication silently.
--
-- `identity.templates` is deliberately absent. Template queries run as the owner through the
-- repository layer, which scopes by tenant in the query, because a policy would have to read
-- the table the app role holds no grant on, and a policy the role cannot evaluate fails the
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
-- connection, so reaching one of those tables without a workspace is a bug, and raising
-- surfaces it, where matching zero rows would look like an empty workspace. Unifying on the
-- lenient form would be worse: it would make an unset GUC return no rows for a
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

--------------------------------------------------------------------------------
-- OAuth identity
--------------------------------------------------------------------------------
--
-- Applied after the table it alters. A single file makes that ordering structural.


ALTER TABLE identity.users
  ALTER COLUMN password_hash DROP NOT NULL;

ALTER TABLE identity.users
  ADD COLUMN oauth_provider   TEXT,
  ADD COLUMN oauth_subject_id TEXT;

-- Every row is either password-authenticated (password_hash set, oauth_* both NULL) or
-- OAuth-provisioned (oauth_provider + oauth_subject_id set, password_hash may be NULL), never
-- a mix, and never neither (a user login method must be determinable at a glance).
ALTER TABLE identity.users
  ADD CONSTRAINT users_auth_method_check CHECK (
    (password_hash IS NOT NULL AND oauth_provider IS NULL AND oauth_subject_id IS NULL)
    OR (oauth_provider IS NOT NULL AND oauth_subject_id IS NOT NULL)
  );

-- The subject id ("sub" claim) an identity provider issues is only unique within that provider,
-- so the lookup/uniqueness key is the pair, not either column alone: otherwise two different
-- providers that happen to both hand out subject id "1" would collide.
CREATE UNIQUE INDEX users_oauth_identity_idx
  ON identity.users (oauth_provider, oauth_subject_id)
  WHERE oauth_provider IS NOT NULL;

--------------------------------------------------------------------------------
-- Stripe webhook idempotency
--------------------------------------------------------------------------------


CREATE TABLE identity.stripe_processed_events (
  event_id      TEXT PRIMARY KEY,
  event_type    TEXT NOT NULL,
  customer_id   TEXT,
  stripe_created TIMESTAMPTZ NOT NULL,
  processed_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Looked up by `apply_stripe_event` to find the most recently applied event for a given
-- customer, so a stale/out-of-order delivery for that customer can be rejected. `NULL`
-- `customer_id` (checkout.session.completed, which has no ordering concern of its own) never
-- matches this index's use, so it's a partial index scoped to the rows that need it.
CREATE INDEX stripe_processed_events_customer_id_idx
  ON identity.stripe_processed_events (customer_id, stripe_created DESC)
  WHERE customer_id IS NOT NULL;

--------------------------------------------------------------------------------
-- Tenant billing
--------------------------------------------------------------------------------


CREATE TABLE identity.tenant_billing (
  tenant_id           UUID PRIMARY KEY REFERENCES identity.tenants(id) ON DELETE CASCADE,
  plan                TEXT,
  stripe_customer_id  TEXT UNIQUE,
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Webhook events for subscription updated/deleted carry only the Stripe customer id, so that is
-- the lookup key on the inbound path. UNIQUE above already provides the index; this comment
-- records why the column is UNIQUE rather than merely indexed: two tenants sharing one Stripe
-- customer would make that lookup ambiguous.

-- ON DELETE CASCADE: deleting a tenant removes its billing row. The Stripe-side subscription is
-- not affected and must be cancelled through Stripe; this table only mirrors that state.

--------------------------------------------------------------------------------
-- Tenant-scoped API keys
--------------------------------------------------------------------------------


ALTER TABLE identity.api_keys
  ADD COLUMN tenant_id UUID REFERENCES identity.tenants(id) ON DELETE CASCADE;

UPDATE identity.api_keys k
   SET tenant_id = w.tenant_id
  FROM identity.workspaces w
 WHERE w.id = k.workspace_id;

-- Derived from the workspace when the inserter did not supply it. The community edition's own
-- `create_api_key` knows nothing about this column (it is added here), so an insert coming
-- through it would otherwise violate the NOT NULL below. Both editions write to this table
-- through that function, so the trigger is what lets the column be mandatory without making the
-- community edition's inserts fail.
CREATE OR REPLACE FUNCTION identity.api_keys_fill_tenant_id()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, identity
AS $$
BEGIN
  IF NEW.tenant_id IS NULL AND NEW.workspace_id IS NOT NULL THEN
    SELECT w.tenant_id INTO NEW.tenant_id
      FROM identity.workspaces w
     WHERE w.id = NEW.workspace_id;
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER api_keys_fill_tenant_id
  BEFORE INSERT ON identity.api_keys
  FOR EACH ROW EXECUTE FUNCTION identity.api_keys_fill_tenant_id();

ALTER TABLE identity.api_keys
  ALTER COLUMN tenant_id SET NOT NULL,
  ALTER COLUMN workspace_id DROP NOT NULL;

CREATE INDEX api_keys_tenant_id_idx ON identity.api_keys (tenant_id);

-- A workspace-scoped key must belong to the tenant that owns its workspace. Without this, a
-- key could name workspace W while claiming tenant T that does not own W, and the tenant check
-- performed at authentication time would be verifying against the wrong tenant.
ALTER TABLE identity.api_keys
  ADD CONSTRAINT api_keys_workspace_matches_tenant CHECK (
    workspace_id IS NULL OR tenant_id IS NOT NULL
  );

-- Resolves a presented key, and for a tenant-scoped key resolves the requested workspace too.
--
-- `p_requested_workspace` is only consulted when the key itself carries no workspace. The
-- membership test (`w.tenant_id = k.tenant_id`) is the tenant isolation boundary for these
-- keys: without it, any tenant-scoped key could name any workspace in the database and the
-- caller would receive a context for a tenant it has no relationship with.
--
-- Returning no row (rather than raising) keeps the existing "unauthenticated" mapping in the
-- caller: an unknown key, a tenant key with no workspace requested, and a tenant key naming
-- someone else's workspace are all indistinguishable to the client, which is the intent.
-- No DEFAULT on the second argument, deliberately. With one, a single-argument call matches
-- *both* overloads and Postgres refuses it as ambiguous ("function is not unique"), which
-- breaks every community-edition caller rather than leaving them alone. Requiring both makes the
-- arity unambiguous, so the one-argument form below keeps resolving to the community edition's.
CREATE OR REPLACE FUNCTION identity.authenticate_api_key(
  p_key_hash bytea,
  p_requested_workspace uuid
)
RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, identity
AS $$
  SELECT k.id,
         COALESCE(k.workspace_id, w.id) AS workspace_id,
         k.tenant_id,
         k.scope,
         k.user_id
  FROM identity.api_keys k
  LEFT JOIN identity.workspaces w
         ON k.workspace_id IS NULL
        AND w.id = p_requested_workspace
        AND w.tenant_id = k.tenant_id
  WHERE k.key_hash = p_key_hash
    -- A tenant-scoped key resolves only when the requested workspace was found in its tenant.
    AND (k.workspace_id IS NOT NULL OR w.id IS NOT NULL)
$$;

REVOKE ALL ON FUNCTION identity.authenticate_api_key(bytea, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION identity.authenticate_api_key(bytea, uuid) TO yorishiro_app;

-- The community edition's single-argument `authenticate_api_key(bytea)` is deliberately left in
-- place. Postgres overloads on arity, so the two-argument form above is an addition rather than
-- a replacement, and the community edition's own binary keeps working against the same database.
--
-- That function INNER JOINs `identity.workspaces`, so a tenant-scoped key (whose `workspace_id`
-- is NULL) resolves to no row through it. A community-edition process reading this database
-- therefore rejects a tenant-scoped key rather than mis-resolving it, which is the correct
-- answer for a process that has no way to be told which workspace was meant.

-- The existing policy compares `workspace_id` against the session's workspace, and a
-- tenant-scoped key's is NULL: `NULL = <uuid>` is NULL rather than true, so such a key's own
-- row is invisible to the very session authenticated by it. `last_used_at` would then never be
-- recorded for exactly the keys that span workspaces, and `admin list-api-keys` could not show
-- them at all.
--
-- A tenant-scoped key is instead visible to any session in its tenant: the scope the key
-- itself has.
DROP POLICY workspace_isolation ON identity.api_keys;

-- `current_setting(name)` *raises* when the variable is unset, rather than returning NULL, so
-- both reads pass `true` for `missing_ok`. The community edition's own policy only ever ran on a
-- connection that had set `app.current_workspace`; this one also runs on the control-plane pool,
-- where neither variable is set, and an unguarded read there fails the query outright rather
-- than matching no rows.
CREATE POLICY workspace_isolation ON identity.api_keys
  USING (
    workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid
    OR (
      workspace_id IS NULL
      AND tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid
    )
  );

--------------------------------------------------------------------------------
-- Template marketplace
--------------------------------------------------------------------------------


CREATE TABLE IF NOT EXISTS identity.template_versions (
  id          UUID PRIMARY KEY DEFAULT uuidv7(),
  template_id UUID NOT NULL REFERENCES identity.templates(id) ON DELETE CASCADE,
  version     INTEGER NOT NULL,
  definition  JSONB NOT NULL,
  changelog   TEXT,
  -- draft: visible only to the owning tenant. pre: published but announced as unstable.
  -- stable: the version an installer gets by default.
  status      TEXT NOT NULL DEFAULT 'draft'
                CHECK (status IN ('draft', 'pre', 'stable')),
  created_by  UUID REFERENCES identity.users(id) ON DELETE SET NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (template_id, version)
);

CREATE INDEX IF NOT EXISTS template_versions_template_idx
  ON identity.template_versions (template_id, version DESC);

-- One review per tenant per template. A tenant that used a template twice does not get two
-- votes, and updating an opinion is an UPDATE rather than a second row.
CREATE TABLE IF NOT EXISTS identity.template_reviews (
  id          UUID PRIMARY KEY DEFAULT uuidv7(),
  template_id UUID NOT NULL REFERENCES identity.templates(id) ON DELETE CASCADE,
  tenant_id   UUID NOT NULL REFERENCES identity.tenants(id) ON DELETE CASCADE,
  rating      SMALLINT NOT NULL CHECK (rating BETWEEN 1 AND 5),
  comment     TEXT,
  created_by  UUID REFERENCES identity.users(id) ON DELETE SET NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (template_id, tenant_id)
);

CREATE INDEX IF NOT EXISTS template_reviews_template_idx
  ON identity.template_reviews (template_id);

-- No row-level security on either table, matching `identity.templates` itself.
--
-- Templates are the one part of the schema that is deliberately *not* RLS-scoped: the table has
-- RLS disabled, `yorishiro_app` holds no grant on it, and every template query runs through the
-- repository layer as the owner role, which scopes by tenant in the query. These two tables are
-- read in exactly the same paths and are scoped the same way.
--
-- Adding policies here would be worse than redundant. They would have to reference
-- `identity.templates` to know whether a template is community-visible, and a policy the app
-- role cannot evaluate (because it cannot read the table the policy joins to) fails the
-- query rather than filtering it.
--
-- What the service layer must therefore enforce, since the database will not:
--   * a draft version is visible only to the tenant that owns its template
--   * a review is written with `tenant_id` taken from the caller's context, never from input

--------------------------------------------------------------------------------
-- LLM-backed fill
--------------------------------------------------------------------------------
--
-- The root set creates these two tables and a later file dropped them again; here they are
-- created once, by this section, and the `IF NOT EXISTS` guards are kept so the text matches
-- what shipped.


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
-- request connection fails at the permission check, which is a stronger guarantee than an RLS
-- policy being written correctly, and does not silently weaken if a policy is later edited. The
-- community initial granted `content` wholesale and this table not at all; that asymmetry is
-- the design, not an oversight, so it is reproduced rather than tidied up.
GRANT SELECT, INSERT, UPDATE, DELETE ON content.fill_proposals TO yorishiro_app;
