-- Billing state for a tenant. Enterprise-only, so it lives here rather than in the vendored
-- community migrations: the community edition has no notion of a subscription, a plan, or a
-- payment processor, and nothing there reads these columns.
--
-- One row per paying tenant, created when checkout first links a Stripe customer. A tenant with
-- no row is unbilled -- the same state a self-hosted deployment is always in -- so the join is
-- LEFT and a missing row means "no plan, no cap", not an error.
--
-- `max_workspaces` deliberately stays on `identity.tenants` (community): the workspace-creation
-- path enforces it, and a self-hosted operator sets it through `admin create-tenant` without any
-- billing involved. Only the columns that name a subscription or a payment processor move here.
CREATE TABLE identity.tenant_billing (
  tenant_id           UUID PRIMARY KEY REFERENCES identity.tenants(id) ON DELETE CASCADE,
  plan                TEXT,
  stripe_customer_id  TEXT UNIQUE,
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Webhook events for subscription updated/deleted carry only the Stripe customer id, so that is
-- the lookup key on the inbound path. UNIQUE above already provides the index; this comment
-- records why the column is UNIQUE rather than merely indexed -- two tenants sharing one Stripe
-- customer would make that lookup ambiguous.

-- ON DELETE CASCADE: deleting a tenant removes its billing row. The Stripe-side subscription is
-- not affected and must be cancelled through Stripe; this table only mirrors that state.
