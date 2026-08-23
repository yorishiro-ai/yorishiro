//! `seed_official_templates` has no HTTP surface (it is a Loco task, `cargo loco task
//! seed_official_templates`), so this calls the service function directly against `ctx.db`,
//! matching how `tests/requests/stripe.rs` calls `billing::` functions directly alongside HTTP
//! requests in the same suite.

use loco_rs::testing::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serial_test::serial;
use yorishiro_core::models::_entities::identity_tenants;
use yorishiro_hosted::HostedApp;
use yorishiro_hosted::services::official_templates::{self, OFFICIAL_TENANT_ID};

/// A first run publishes every built-in template and creates the official tenant; a second run
/// with nothing changed republishes nothing, which is what makes it safe to run on every
/// deployment restart rather than once ever.
#[tokio::test]
#[serial]
async fn seeding_is_idempotent_and_creates_the_official_tenant() {
    request_with_create_db::<HostedApp, _, _>(|_request, ctx| async move {
        let built_in_count = yorishiro_core::templates::list_templates().len();

        let first = official_templates::seed_official_templates(&ctx)
            .await
            .expect("first seed run");
        assert_eq!(first.published.len(), built_in_count);
        assert_eq!(first.updated.len(), 0);
        assert_eq!(first.unchanged.len(), 0);

        let tenant = identity_tenants::Entity::find()
            .filter(identity_tenants::Column::Id.eq(OFFICIAL_TENANT_ID))
            .one(&ctx.db)
            .await
            .unwrap();
        assert!(
            tenant.is_some(),
            "the official tenant row must exist after seeding"
        );

        let second = official_templates::seed_official_templates(&ctx)
            .await
            .expect("second seed run");
        assert_eq!(
            second.published.len(),
            0,
            "an unchanged built-in must not be published again"
        );
        assert_eq!(second.updated.len(), 0);
        assert_eq!(second.unchanged.len(), built_in_count);

        super::close_app_pools(&ctx).await;
    })
    .await;
}
