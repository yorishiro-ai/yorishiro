-- The Sqlite tier's schema, transcribed from `migrations/20260814000000_initial.sql`'s final
-- shape (that file's own later sections ALTER several of these tables; this states each table
-- once, already in its final form, the way that file's own header explains for its own reason).
--
-- What does not carry over, and why:
--
--   * Extensions, `CREATE SCHEMA`, roles, GRANTs, RLS `ENABLE`/policies: Sqlite has none of
--     these. Tenant isolation on this engine is the process boundary itself, not a
--     database-enforced policy.
--   * `identity.authenticate_api_key`: Sqlite cannot hold a SECURITY DEFINER function; its
--     replacement is an ordinary query.
--   * `uuidv7()` column defaults: the application generates every id on this engine instead
--     (`db::Engine::generated_id`), so no table here has a default on `id`.
--   * GIN/HNSW/trgm indexes, `ALTER ... TYPE vector`: vector/trigram search's Sqlite
--     replacement (sqlite-vec, FTS5) is not yet decided.
--
-- Schema-qualified names do not exist for a single-file database, so every table below is bare
-- (`tenants`, not `identity.tenants`), matching `Engine::schema_table`'s Sqlite rendering.
--
-- Bare-name collision check (2026-08-21): `identity.*` and `content.*` in the source file share
-- no table name once flattened.
-- Confirmed by sorting every `CREATE TABLE` name and finding no duplicate.

CREATE TABLE tenants (
  id             BLOB PRIMARY KEY,
  name           TEXT NOT NULL,
  max_workspaces INTEGER,
  created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Final shape: password_hash nullable and the two oauth_* columns, from
-- `migrations/20260814000000_initial.sql`'s later "OAuth identity" section.
CREATE TABLE users (
  id              BLOB PRIMARY KEY,
  email           TEXT NOT NULL UNIQUE,
  password_hash   TEXT,
  display_name    TEXT,
  oauth_provider  TEXT,
  oauth_subject_id TEXT,
  created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  CHECK (
    (password_hash IS NOT NULL AND oauth_provider IS NULL AND oauth_subject_id IS NULL)
    OR (oauth_provider IS NOT NULL AND oauth_subject_id IS NOT NULL)
  )
);

CREATE UNIQUE INDEX users_oauth_identity_idx
  ON users (oauth_provider, oauth_subject_id)
  WHERE oauth_provider IS NOT NULL;

CREATE TABLE tenant_memberships (
  id         BLOB PRIMARY KEY,
  tenant_id  BLOB NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  user_id    BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role       TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (tenant_id, user_id)
);

-- `schema_id` is inline here (not added by a later `ALTER`, unlike the Postgres file): Sqlite's
-- `ALTER TABLE ... ADD COLUMN ... REFERENCES` cannot reference a table that does not exist yet
-- either, so the same circularity applies.
-- Stating the final shape once sidesteps it rather than reproducing the two-pass history.
CREATE TABLE workspaces (
  id                   BLOB PRIMARY KEY,
  tenant_id            BLOB NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  name                 TEXT NOT NULL,
  max_entities         INTEGER,
  status               TEXT NOT NULL DEFAULT 'schema_pending'
                          CHECK (status IN ('schema_pending', 'active')),
  embedding_model      TEXT,
  embedding_dimensions INTEGER CHECK (embedding_dimensions IS NULL OR embedding_dimensions > 0),
  schema_id            BLOB REFERENCES schemas(id),
  created_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (tenant_id, name)
);

-- Final shape: `tenant_id` (from the later "Tenant-scoped API keys" section), NOT NULL there,
-- `workspace_id` nullable to match. The `api_keys_fill_tenant_id` trigger that backfills it on
-- Postgres has no Sqlite counterpart to backfill from, since this is a fresh table on this
-- engine; every insert on this engine supplies `tenant_id` directly instead.
CREATE TABLE api_keys (
  id           BLOB PRIMARY KEY,
  workspace_id BLOB REFERENCES workspaces(id) ON DELETE CASCADE,
  tenant_id    BLOB NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  user_id      BLOB REFERENCES users(id) ON DELETE SET NULL,
  key_hash     BLOB NOT NULL UNIQUE,
  key_prefix   TEXT NOT NULL,
  scope        TEXT NOT NULL CHECK (scope IN ('read', 'write', 'schema', 'migration')),
  created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  last_used_at TEXT,
  CHECK (workspace_id IS NULL OR tenant_id IS NOT NULL)
);

CREATE INDEX api_keys_tenant_id_idx ON api_keys (tenant_id);

CREATE TABLE invites (
  id         BLOB PRIMARY KEY,
  tenant_id  BLOB NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  email      TEXT NOT NULL,
  role       TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
  token_hash BLOB NOT NULL UNIQUE,
  expires_at TEXT NOT NULL,
  used_at    TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX invites_tenant_id_idx ON invites (tenant_id);

-- `tags TEXT[]` has no Sqlite equivalent: stored as a JSON array instead. The code that reads
-- and writes it (`models/tenancy/template_library`) still binds a Postgres array today, so this
-- column is unused until that genericization lands.
CREATE TABLE templates (
  id          BLOB PRIMARY KEY,
  tenant_id   BLOB NOT NULL REFERENCES tenants(id),
  name        TEXT NOT NULL,
  description TEXT,
  definition  TEXT NOT NULL,
  tags        TEXT NOT NULL DEFAULT '[]',
  locale      TEXT,
  visibility  TEXT NOT NULL DEFAULT 'tenant' CHECK (visibility IN ('tenant', 'community')),
  author      TEXT,
  fork_of     BLOB REFERENCES templates(id) ON DELETE SET NULL,
  created_by  BLOB REFERENCES users(id),
  created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (tenant_id, name)
);

CREATE INDEX templates_tenant_id_idx ON templates (tenant_id);

-- Ported as a trigger, matching `content.detach_orphaned_schema_origin` on Postgres: deleting a
-- template must not destroy schemas copied from it, and must stop them claiming to follow
-- something that is gone.
CREATE TRIGGER templates_detach_schema_origins
  BEFORE DELETE ON templates
  FOR EACH ROW
BEGIN
  UPDATE schemas
     SET origin_status = 'detached'
   WHERE origin_template_id = OLD.id
     AND origin_status = 'linked';
END;

-- BOOLEAN PRIMARY KEY has no Sqlite equivalent; `INTEGER PRIMARY KEY CHECK (id = 1)` is the same
-- single-row guarantee.
CREATE TABLE maintenance (
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  mode        TEXT NOT NULL DEFAULT 'off' CHECK (mode IN ('off', 'read_only', 'full_lock')),
  retry_after INTEGER NOT NULL DEFAULT 300 CHECK (retry_after > 0),
  reason      TEXT,
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

INSERT INTO maintenance (id) VALUES (1);

CREATE TABLE workspace_llm_keys (
  workspace_id BLOB PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  base_url     TEXT NOT NULL,
  model        TEXT NOT NULL,
  api_key      TEXT NOT NULL,
  created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE schemas (
  id                 BLOB PRIMARY KEY,
  tenant_id          BLOB NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  workspace_id       BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name               TEXT NOT NULL,
  version            INTEGER NOT NULL DEFAULT 1,
  definition         TEXT NOT NULL,
  status             TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
  origin_template_id BLOB REFERENCES templates(id) ON DELETE SET NULL,
  origin_status      TEXT NOT NULL DEFAULT 'detached'
                        CHECK (origin_status IN ('linked', 'detached')),
  origin_snapshot    TEXT,
  created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  CONSTRAINT schemas_workspace_name_version_key UNIQUE (workspace_id, name, version)
);

CREATE INDEX schemas_tenant_id_idx    ON schemas (tenant_id);
CREATE INDEX schemas_workspace_id_idx ON schemas (workspace_id);
CREATE INDEX schemas_origin_template_idx
  ON schemas (origin_template_id)
  WHERE origin_template_id IS NOT NULL;

-- `embedding vector(768)` has no Sqlite equivalent yet: plain nullable BLOB until a vector
-- extension (sqlite-vec) decides the real representation.
CREATE TABLE entities (
  id             BLOB PRIMARY KEY,
  workspace_id   BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  schema_id      BLOB NOT NULL REFERENCES schemas(id),
  schema_version INTEGER NOT NULL,
  entity_type    TEXT NOT NULL,
  data           TEXT NOT NULL,
  embedding      BLOB,
  created_by     BLOB REFERENCES users(id) ON DELETE SET NULL,
  updated_by     BLOB REFERENCES users(id) ON DELETE SET NULL,
  created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX entities_workspace_type_idx ON entities (workspace_id, entity_type, created_at);

CREATE TABLE relations (
  id            BLOB PRIMARY KEY,
  workspace_id  BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  source_id     BLOB NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
  target_id     BLOB NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
  relation_type TEXT NOT NULL,
  properties    TEXT NOT NULL DEFAULT '{}',
  status        TEXT NOT NULL DEFAULT 'active'
                  CHECK (status IN ('active', 'deprecated', 'archived')),
  created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (workspace_id, source_id, target_id, relation_type)
);

CREATE INDEX relations_source_idx ON relations (workspace_id, source_id, status);
CREATE INDEX relations_target_idx ON relations (workspace_id, target_id, status);

CREATE TABLE entity_snapshots (
  id             BLOB PRIMARY KEY,
  job_id         BLOB NOT NULL,
  workspace_id   BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  entity_id      BLOB NOT NULL,
  schema_id      BLOB NOT NULL,
  schema_version INTEGER NOT NULL,
  data           TEXT NOT NULL,
  created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX entity_snapshots_job_idx    ON entity_snapshots (workspace_id, job_id);
CREATE INDEX entity_snapshots_entity_idx ON entity_snapshots (workspace_id, entity_id, created_at DESC);

CREATE TABLE fill_proposals (
  id           BLOB PRIMARY KEY,
  job_id       BLOB NOT NULL,
  workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  entity_id    BLOB NOT NULL,
  field_name   TEXT NOT NULL,
  proposed     TEXT NOT NULL,
  created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (workspace_id, job_id, entity_id, field_name)
);

CREATE INDEX fill_proposals_job_idx ON fill_proposals (workspace_id, job_id);

CREATE TABLE entity_column_preferences (
  id           BLOB PRIMARY KEY,
  workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  entity_type  TEXT NOT NULL,
  columns      TEXT NOT NULL DEFAULT '[]',
  created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (workspace_id, entity_type),
  CONSTRAINT entity_column_preferences_columns_is_array
    CHECK (json_valid(columns) AND json_type(columns) = 'array')
);

CREATE TABLE stripe_processed_events (
  event_id       TEXT PRIMARY KEY,
  event_type     TEXT NOT NULL,
  customer_id    TEXT,
  stripe_created TEXT NOT NULL,
  processed_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX stripe_processed_events_customer_id_idx
  ON stripe_processed_events (customer_id, stripe_created DESC)
  WHERE customer_id IS NOT NULL;

CREATE TABLE tenant_billing (
  tenant_id          BLOB PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
  plan               TEXT,
  stripe_customer_id TEXT UNIQUE,
  created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE template_versions (
  id          BLOB PRIMARY KEY,
  template_id BLOB NOT NULL REFERENCES templates(id) ON DELETE CASCADE,
  version     INTEGER NOT NULL,
  definition  TEXT NOT NULL,
  changelog   TEXT,
  status      TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'pre', 'stable')),
  created_by  BLOB REFERENCES users(id) ON DELETE SET NULL,
  created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (template_id, version)
);

CREATE INDEX template_versions_template_idx ON template_versions (template_id, version DESC);

CREATE TABLE template_reviews (
  id          BLOB PRIMARY KEY,
  template_id BLOB NOT NULL REFERENCES templates(id) ON DELETE CASCADE,
  tenant_id   BLOB NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  rating      INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
  comment     TEXT,
  created_by  BLOB REFERENCES users(id) ON DELETE SET NULL,
  created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (template_id, tenant_id)
);

CREATE INDEX template_reviews_template_idx ON template_reviews (template_id);
