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
            // inject-above (do not remove this comment)
        ]
    }
}
