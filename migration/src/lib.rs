#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
pub use sea_orm_migration::prelude::*;
mod helpers;
mod m20261020_000000_create_tenants;
mod m20261020_000001_create_users;
mod m20261020_000002_create_tenant_memberships;
mod m20261020_000003_create_workspaces;
mod m20261020_000004_create_workspace_invites;
mod m20261020_000005_create_templates;
mod m20261020_000006_create_schemas;
mod m20261020_000007_create_api_keys;
mod m20261020_000008_create_entities;
mod m20261020_000009_create_entity_relations;
mod m20261020_000010_create_entity_snapshots;
mod m20261020_000011_create_billing;
mod m20261020_000012_create_template_catalog;
mod m20261020_000013_create_workspace_credentials;
mod m20261020_000014_create_entity_column_preferences;
mod m20261020_000015_create_api_key_audit_log;
mod m20261020_000016_create_maintenance;
mod m20261020_000017_create_reindex_schedules;
mod m20261020_000018_create_compute_credit_ledger;
mod m20261020_000019_create_workspace_schema_forks;
mod m20261020_000020_create_workspace_schema_fork_heads;
mod m20261020_000021_create_inference_jobs;
mod m20261020_000022_create_inference_proposals;
mod m20261020_000023_create_queue_job_lifecycles;
mod m20261020_000024_create_queue_job_dispatch_outbox;
mod m20261020_000025_create_query_embedding_requests;
mod m20261020_000026_create_timezone_state;
mod m20261020_000027_create_entity_embeddings;
mod m20261028_160610_workspace_memberships;
mod m20261029_040619_add_tenant_disabled_at;
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20261020_000000_create_tenants::Migration),
            Box::new(m20261020_000001_create_users::Migration),
            Box::new(m20261020_000002_create_tenant_memberships::Migration),
            Box::new(m20261020_000003_create_workspaces::Migration),
            Box::new(m20261020_000004_create_workspace_invites::Migration),
            Box::new(m20261020_000005_create_templates::Migration),
            Box::new(m20261020_000006_create_schemas::Migration),
            Box::new(m20261020_000007_create_api_keys::Migration),
            Box::new(m20261020_000008_create_entities::Migration),
            Box::new(m20261020_000009_create_entity_relations::Migration),
            Box::new(m20261020_000010_create_entity_snapshots::Migration),
            Box::new(m20261020_000011_create_billing::Migration),
            Box::new(m20261020_000012_create_template_catalog::Migration),
            Box::new(m20261020_000013_create_workspace_credentials::Migration),
            Box::new(m20261020_000014_create_entity_column_preferences::Migration),
            Box::new(m20261020_000015_create_api_key_audit_log::Migration),
            Box::new(m20261020_000016_create_maintenance::Migration),
            Box::new(m20261020_000017_create_reindex_schedules::Migration),
            Box::new(m20261020_000018_create_compute_credit_ledger::Migration),
            Box::new(m20261020_000019_create_workspace_schema_forks::Migration),
            Box::new(m20261020_000020_create_workspace_schema_fork_heads::Migration),
            Box::new(m20261020_000021_create_inference_jobs::Migration),
            Box::new(m20261020_000022_create_inference_proposals::Migration),
            Box::new(m20261020_000023_create_queue_job_lifecycles::Migration),
            Box::new(m20261020_000024_create_queue_job_dispatch_outbox::Migration),
            Box::new(m20261020_000025_create_query_embedding_requests::Migration),
            Box::new(m20261020_000026_create_timezone_state::Migration),
            Box::new(m20261020_000027_create_entity_embeddings::Migration),
            Box::new(m20261028_160610_workspace_memberships::Migration),
            Box::new(m20261029_040619_add_tenant_disabled_at::Migration),
            // inject-above (do not remove this comment)
        ]
    }
}
