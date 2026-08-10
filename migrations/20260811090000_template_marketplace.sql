-- Template marketplace.
--
-- `identity.templates` already carries `visibility` ('tenant' | 'community') and `fork_of`, which
-- is what a tenant sharing a template and another tenant forking it need. What is missing is the
-- part that makes sharing safe to consume: which versions exist, which of them are fit to use,
-- and what other tenants found when they tried one.

-- A published snapshot of a template. The template row keeps moving as its owner edits it; a
-- version does not, so a tenant that installs one gets the definition it actually looked at.
CREATE TABLE identity.template_versions (
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

CREATE INDEX template_versions_template_idx
  ON identity.template_versions (template_id, version DESC);

-- One review per tenant per template. A tenant that used a template twice does not get two
-- votes, and updating an opinion is an UPDATE rather than a second row.
CREATE TABLE identity.template_reviews (
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

CREATE INDEX template_reviews_template_idx ON identity.template_reviews (template_id);

-- No row-level security on either table, matching `identity.templates` itself.
--
-- Templates are the one part of the schema that is deliberately *not* RLS-scoped: the table has
-- RLS disabled, `yorishiro_app` holds no grant on it, and every template query runs through the
-- repository layer as the owner role, which scopes by tenant in the query. These two tables are
-- read in exactly the same paths and are scoped the same way.
--
-- Adding policies here would be worse than redundant. They would have to reference
-- `identity.templates` to know whether a template is community-visible, and a policy the app
-- role cannot evaluate -- because it cannot read the table the policy joins to -- fails the
-- query rather than filtering it.
--
-- What the repository layer must therefore enforce, since the database will not:
--   * a draft version is visible only to the tenant that owns its template
--   * a review is written with `tenant_id` taken from the caller's context, never from input
