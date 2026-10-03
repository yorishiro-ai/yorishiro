# Clock-Dependent Decisions

This inventory records wall-clock calls in the application and why only selected decision points use a local timestamp seam.

## Selected seams

| Site | Decision | Seam and boundary coverage |
|---|---|---|
| `ee/services/oauth/state_token.rs` | OAuth `state` issued-at and freshness validation | `issue_at` and `verify_at` accept the timestamp locally; tests pin the 600-second expiry boundary and the five-second future-skew boundary. |
| `ee/controllers/stripe.rs` | Stripe signature timestamp tolerance | The existing `verify_at` seam accepts the inbound verification time; tests pin both inclusive 300-second edges and both outside cases. |
| `ee/controllers/middleware/edition.rs` | Per-request licence expiry | The existing `is_active_at` fold accepts Unix seconds; tests pin exclusive `exp > now` behavior. |
| `src/models/tenancy.rs` | Invitation issuance and redemption expiry | Private `create_invite_at` and `redeem_invite_at` helpers accept one injected UTC value; backend tests pin `expires_at == now` as expired, just-before-expiry as valid, just-after-expiry as expired, and the TTL calculation. |
| `ee/tasks/reindex_scheduler.rs` | Reindex schedule due-time decision | `run_at` receives the tick and `is_due` uses `scheduled_for <= tick`; tests cover offsets -301, 0, +1, +300, and +301 seconds. The five-minute value schedules the next run and is not a missed-run grace window. |

Production wrappers use `chrono::Utc::now()` and retain the existing windows, response behavior, logs, gates, queue flow, and persistence operations.

## Inventory and deferrals

| Call sites | Classification | Reason for deferral |
|---|---|---|
| `src/db.rs`; `src/models/compute_credit_ledger.rs`, `schema_schemas.rs`, `system_maintenance.rs`, `tenant_reindex_schedules.rs`, and `workspace_schema_forks.rs`; `ee/models/billing.rs`, `compute_credit_ledger.rs`, `embedding_keys.rs`, `entity_columns.rs`, `inference_jobs.rs`, `llm_keys.rs`, `marketplace.rs`, `schema_schemas.rs`, `template_templates.rs`, `worker_classes.rs`, and `workspace_schema_forks.rs`; `src/models/api_keys.rs`; and `migration/src/helpers.rs` plus `migration/src/m20260829_000000_initial_schema/schema.rs` | Persistence timestamps such as `created_at`, `updated_at`, `last_used_at`, and origin timestamps | Database and model timestamp semantics are persistence concerns, not externally observable expiry decisions; abstracting them would broaden the seam and risk changing transactions or backend defaults. |
| `src/controllers/middleware/access_log.rs`, `src/initializers/db_load_guard.rs`, `ee/tasks/sqlite_ann_benchmark.rs` | Monotonic elapsed-time measurements | These values measure runtime duration and do not decide wall-clock expiry or due dates; `Instant` is already the correct source. |
| `src/controllers/middleware/rate_limit.rs` | Monotonic quota-window measurement | The limiter uses elapsed duration rather than calendar time, and no deterministic behavior test requires replacing its monotonic source. |
| `src/services/embedding/model_fetch.rs` | Filesystem mtime age for stale partial downloads | The production decision is based on filesystem metadata and the tests set mtime directly; a clock seam would not improve the external behavior under test. |

No process-wide clock, service locator, database timestamp abstraction, or `Instant` replacement is introduced.
