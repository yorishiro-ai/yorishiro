//! The model layer for the tables this crate owns: record shapes and the queries that read and write them, together.
//!
//! Base's own models are reached through `crate::models`; most modules here are the ones whose tables this crate's own migrations add.
//! `origin` is the exception: it owns no table, and reads base's own `schema_schemas`/`template_templates` on `ctx.db`, since the endpoint it serves is enterprise regardless of which tables it happens to read.

pub mod api_keys;
pub mod compute_credit_ledger;
pub mod entity_column_preferences;
pub mod entity_entities;
pub mod inference_jobs;
pub mod inference_proposals;
pub mod marketplace;
pub mod schema_schemas;
pub mod stripe_events;
pub mod template_templates;
pub mod tenant_billing;
pub(crate) mod tenant_tenants;
pub mod user_users;
pub mod workspace_embedding_keys;
pub(crate) mod workspace_llm_keys;
pub mod workspace_schema_forks;
pub mod workspace_worker_classes;
