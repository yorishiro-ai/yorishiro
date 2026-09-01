#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
pub use sea_orm_migration::prelude::*;

mod helpers;
mod m20260829_000000_initial_schema;
mod m20260829_000001_redeclare_embedding_width;
mod m20260831_000001_tenant_embedding_defaults;
mod m20260901_000001_add_reindex_embeddings_to_audit_log_action_check;
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260829_000000_initial_schema::Migration),
            Box::new(m20260829_000001_redeclare_embedding_width::Migration),
            Box::new(m20260831_000001_tenant_embedding_defaults::Migration),
            Box::new(m20260901_000001_add_reindex_embeddings_to_audit_log_action_check::Migration),
            // inject-above (do not remove this comment)
        ]
    }
}
