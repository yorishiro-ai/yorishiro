#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
pub use sea_orm_migration::prelude::*;
mod helpers;

mod m20260829_000000_initial_schema;
mod m20260909_000001_embedding_width_partitions;
mod m20260911_000002_add_tenant_memberships_user_id_index;
mod m20260912_000003_schema_origin_updated_at;
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260829_000000_initial_schema::Migration),
            Box::new(m20260909_000001_embedding_width_partitions::Migration),
            Box::new(m20260911_000002_add_tenant_memberships_user_id_index::Migration),
            Box::new(m20260912_000003_schema_origin_updated_at::Migration),
            // inject-above (do not remove this comment)
        ]
    }
}
