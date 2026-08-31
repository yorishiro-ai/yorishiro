#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
pub use sea_orm_migration::prelude::*;

mod helpers;
mod m20260829_000000_initial_schema;
mod m20260829_000001_redeclare_embedding_width;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        // inject-above
        vec![
            Box::new(m20260829_000000_initial_schema::Migration),
            Box::new(m20260829_000001_redeclare_embedding_width::Migration),
        ]
    }
}
