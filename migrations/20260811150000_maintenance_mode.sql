-- Deployment-wide maintenance state.
--
-- Kept in the database rather than in the process so every node reads the same value: a flag
-- held in memory would put one replica in maintenance while its siblings kept accepting
-- writes, which is the opposite of what the mode is for.
--
-- A single row, enforced by the CHECK on the primary key. There is one deployment state, not
-- one per tenant -- this exists for schema migrations and restores, which stop everything.
CREATE TABLE identity.maintenance (
  id          BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
  -- 'off' serves normally, 'read_only' refuses writes with 423, 'full_lock' refuses
  -- everything with 503.
  mode        TEXT NOT NULL DEFAULT 'off'
              CHECK (mode IN ('off', 'read_only', 'full_lock')),
  -- Seconds for the Retry-After header. Agents retry on the header rather than on the body,
  -- so a mode with no hint invites an immediate retry loop.
  retry_after INTEGER NOT NULL DEFAULT 300 CHECK (retry_after > 0),
  -- Shown to callers. An operator saying "restoring from backup, back by 09:00" saves the
  -- support question the bare status code provokes.
  reason      TEXT,
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO identity.maintenance (id) VALUES (TRUE);

-- Readable by the request role: every request consults it, and it holds nothing tenant-scoped
-- to leak. Writes stay with the migration role, which is what the admin CLI and the REST route
-- connect as -- turning maintenance on is an operator action, not a tenant one.
GRANT SELECT ON identity.maintenance TO yorishiro_app;
