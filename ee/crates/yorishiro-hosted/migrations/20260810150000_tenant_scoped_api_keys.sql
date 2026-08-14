-- Tenant-scoped API keys. Hosted-only, so it lives here rather than in the vendored community
-- migrations: the community edition binds a key to exactly one workspace and has no notion of
-- choosing one per request.
--
-- An API key has always been bound to exactly one workspace, so a client working across several
-- workspaces had to hold (and swap between) one key per workspace. A key with a NULL
-- workspace_id is instead scoped to its tenant, and the workspace is named per request with the
-- `X-Workspace-Id` header.
--
-- workspace_id is therefore nullable, and tenant_id becomes a column of its own: for a
-- workspace-scoped key the tenant was reachable by joining through the workspace, but a
-- tenant-scoped key has no workspace to join through.

ALTER TABLE identity.api_keys
  ADD COLUMN tenant_id UUID REFERENCES identity.tenants(id) ON DELETE CASCADE;

UPDATE identity.api_keys k
   SET tenant_id = w.tenant_id
  FROM identity.workspaces w
 WHERE w.id = k.workspace_id;

-- Derived from the workspace when the inserter did not supply it. The community edition's own
-- `create_api_key` knows nothing about this column -- it is added here -- so an insert coming
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
-- *both* overloads and Postgres refuses it as ambiguous ("function is not unique") -- which
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
-- That function INNER JOINs `identity.workspaces`, so a tenant-scoped key -- whose `workspace_id`
-- is NULL -- resolves to no row through it. A community-edition process reading this database
-- therefore rejects a tenant-scoped key rather than mis-resolving it, which is the correct
-- answer for a process that has no way to be told which workspace was meant.

-- The existing policy compares `workspace_id` against the session's workspace, and a
-- tenant-scoped key's is NULL -- `NULL = <uuid>` is NULL rather than true, so such a key's own
-- row is invisible to the very session authenticated by it. `last_used_at` would then never be
-- recorded for exactly the keys that span workspaces, and `admin list-api-keys` could not show
-- them at all.
--
-- A tenant-scoped key is instead visible to any session in its tenant -- the scope the key
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
