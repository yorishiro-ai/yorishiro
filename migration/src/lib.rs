#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
pub use sea_orm_migration::prelude::*;

mod helpers;
mod m20260822_100000_tenants;
mod m20260822_100100_users;
mod m20260822_100200_tenant_memberships;
mod m20260822_100300_workspaces;
mod m20260822_100400_api_keys;
mod m20260822_100500_invites;
mod m20260822_100600_templates;
mod m20260822_100700_maintenance;
mod m20260822_100800_content_schemas;
mod m20260822_100900_content_entities;
mod m20260822_101000_content_relations;
mod m20260822_101100_content_entity_snapshots;
mod m20260822_101200_authenticate_api_key;
mod m20260823_100000_tenant_billing;
mod m20260823_100100_stripe_processed_events;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260822_100000_tenants::Migration),
            Box::new(m20260822_100100_users::Migration),
            Box::new(m20260822_100200_tenant_memberships::Migration),
            Box::new(m20260822_100300_workspaces::Migration),
            Box::new(m20260822_100400_api_keys::Migration),
            Box::new(m20260822_100500_invites::Migration),
            Box::new(m20260822_100600_templates::Migration),
            Box::new(m20260822_100700_maintenance::Migration),
            Box::new(m20260822_100800_content_schemas::Migration),
            Box::new(m20260822_100900_content_entities::Migration),
            Box::new(m20260822_101000_content_relations::Migration),
            Box::new(m20260822_101100_content_entity_snapshots::Migration),
            Box::new(m20260822_101200_authenticate_api_key::Migration),
            Box::new(m20260823_100000_tenant_billing::Migration),
            Box::new(m20260823_100100_stripe_processed_events::Migration),
            // inject-above (do not remove this comment)
        ]
    }
}
