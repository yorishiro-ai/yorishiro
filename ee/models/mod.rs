//! Enterprise model operations, mirroring the base model paths for shared functionality.
//!
//! Base models are reached through `crate::models`; schema origin operations live under `schema_schemas::origin`.

// Billing and plans.
pub mod compute_credit_ledger;
pub mod stripe_events;
pub mod tenant_billing;

// Schemas, forks and the marketplace.
pub mod marketplace;
pub mod schema_schemas;
pub mod template_templates;
pub mod workspace_schema_forks;

// Inference and entity extensions.
pub mod entity_column_preferences;
pub mod entity_entities;
pub mod inference_jobs;
pub mod inference_proposals;

// Per-workspace provider keys and worker classes.
pub mod workspace_embedding_keys;
pub mod workspace_llm_keys;
pub mod workspace_worker_classes;

// Tenants, users, keys and access.
pub mod access_keys;
pub(crate) mod iam;
pub mod tenant_reindex_schedules;
pub mod user_users;
