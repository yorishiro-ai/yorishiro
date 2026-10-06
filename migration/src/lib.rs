#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
pub use sea_orm_migration::prelude::*;
mod helpers;

mod m20260829_000000_initial_schema;
mod m20260909_000001_embedding_width_partitions;
mod m20260911_000002_add_tenant_memberships_user_id_index;
mod m20260912_000003_schema_origin_updated_at;
mod m20260912_000004_compute_credit_ledger;
mod m20260912_000005_fill_defaults_audit_action;
mod m20260913_000006_inference_jobs;
mod m20260914_000007_workspace_schema_forks;
mod m20260929_000008_inference_proposals;
mod m20260930_000009_queue_job_lifecycle;
mod m20260930_000010_queue_job_admission;
mod m20260930_000011_queue_job_lease;
mod m20260930_000012_inference_job_attempt;
mod m20260930_000013_inference_job_attempt_privileges;
mod m20261001_000014_queue_lifecycle_starvation_index;
mod m20261002_000015_api_key_name;
mod m20261005_000016_add_entity_embedding_sync_token;
mod m20261005_000017_embedding_tables_tenant_read;
mod m20261006_000018_query_embedding_requests;
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260829_000000_initial_schema::Migration),
            Box::new(m20260909_000001_embedding_width_partitions::Migration),
            Box::new(m20260911_000002_add_tenant_memberships_user_id_index::Migration),
            Box::new(m20260912_000003_schema_origin_updated_at::Migration),
            Box::new(m20260912_000004_compute_credit_ledger::Migration),
            Box::new(m20260912_000005_fill_defaults_audit_action::Migration),
            Box::new(m20260913_000006_inference_jobs::Migration),
            Box::new(m20260914_000007_workspace_schema_forks::Migration),
            Box::new(m20260929_000008_inference_proposals::Migration),
            Box::new(m20260930_000009_queue_job_lifecycle::Migration),
            Box::new(m20260930_000010_queue_job_admission::Migration),
            Box::new(m20260930_000011_queue_job_lease::Migration),
            Box::new(m20260930_000012_inference_job_attempt::Migration),
            Box::new(m20260930_000013_inference_job_attempt_privileges::Migration),
            Box::new(m20261001_000014_queue_lifecycle_starvation_index::Migration),
            Box::new(m20261002_000015_api_key_name::Migration),
            Box::new(m20261005_000016_add_entity_embedding_sync_token::Migration),
            Box::new(m20261005_000017_embedding_tables_tenant_read::Migration),
            Box::new(m20261006_000018_query_embedding_requests::Migration),
            // inject-above (do not remove this comment)
        ]
    }
}
