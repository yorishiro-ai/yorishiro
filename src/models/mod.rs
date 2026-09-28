pub mod _entities;
mod content;
mod identity;
pub mod pagination;
mod system;
mod templates;

// Keep the pre-layout paths stable for REST, MCP, workers, tests, and the EE module.
// These path declarations compile moved sources once and keep Loco's post-processor from recreating them.
#[path = "identity/api_key_audit_log.rs"]
pub mod api_key_audit_log;
#[path = "identity/api_keys.rs"]
pub mod api_keys;
#[path = "system/compute_credit_ledger.rs"]
pub mod compute_credit_ledger;
#[path = "content/entity_column_preferences.rs"]
pub mod entity_column_preferences;
#[path = "content/entity_embeddings.rs"]
pub mod entity_embeddings;
#[path = "content/entity_entities/mod.rs"]
pub mod entity_entities;
#[path = "content/entity_relations.rs"]
pub mod entity_relations;
#[path = "content/entity_snapshots.rs"]
pub mod entity_snapshots;
#[path = "content/export.rs"]
pub mod export;
#[path = "content/import.rs"]
pub mod import;
#[path = "system/inference_jobs.rs"]
pub mod inference_jobs;
#[path = "content/recall.rs"]
pub mod recall;
#[path = "content/schema_schemas.rs"]
pub mod schema_schemas;
#[path = "content/search.rs"]
pub mod search;
#[path = "system/stripe_events.rs"]
pub mod stripe_events;
#[path = "system/system_maintenance.rs"]
pub mod system_maintenance;
#[path = "templates/template_reviews.rs"]
pub mod template_reviews;
#[path = "templates/template_templates.rs"]
pub mod template_templates;
#[path = "templates/template_versions.rs"]
pub mod template_versions;
#[path = "identity/tenancy.rs"]
pub mod tenancy;
#[path = "system/tenant_billing.rs"]
pub mod tenant_billing;
#[path = "identity/tenant_memberships.rs"]
pub mod tenant_memberships;
#[path = "system/tenant_reindex_schedules.rs"]
pub mod tenant_reindex_schedules;
#[path = "identity/tenant_tenants.rs"]
pub mod tenant_tenants;
#[path = "identity/user_users.rs"]
pub mod user_users;
#[path = "identity/workspace_embedding_keys.rs"]
pub mod workspace_embedding_keys;
#[path = "identity/workspace_invites.rs"]
pub mod workspace_invites;
#[path = "identity/workspace_llm_keys.rs"]
pub mod workspace_llm_keys;
#[path = "content/workspace_schema_fork_heads.rs"]
pub mod workspace_schema_fork_heads;
#[path = "content/workspace_schema_forks.rs"]
pub mod workspace_schema_forks;
#[path = "system/workspace_worker_classes.rs"]
pub mod workspace_worker_classes;
#[path = "identity/workspace_workspaces.rs"]
pub mod workspace_workspaces;
