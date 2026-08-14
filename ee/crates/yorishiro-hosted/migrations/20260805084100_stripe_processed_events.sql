-- Idempotency and ordering guard for the Stripe webhook. Stripe does not guarantee delivery
-- order and retries on a slow/failed response, so `apply_stripe_event`
-- (http::controllers::stripe) records every event id it has successfully applied here and skips
-- any it has already seen (`event_id` is the primary key). For events tied to a Stripe customer
-- (subscription created/updated/deleted), it also skips one whose own `created` timestamp is
-- older than the last-applied event's for that same customer -- a delayed/retried delivery of a
-- stale event must not undo a newer one that already landed.
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
