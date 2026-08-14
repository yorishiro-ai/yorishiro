//! Shared test-only helpers. `combined_migrator` in particular exists because `sqlx::test`'s
//! `migrations = "<path>"` argument accepts only a single directory, but this repo's database
//! needs two: the vendored community edition's migrations (`identity.users` etc.) and this
//! repo's own enterprise-only ones (OAuth's `oauth_provider`/`oauth_subject_id` columns). Tests
//! that exercise OAuth-provisioned users need both applied to their ephemeral `sqlx::test`
//! database, the same way `yorishiro-hosted-server`'s `main` applies both to the real one.

use crate::http::controllers::stripe::StripeConfig;
use crate::state::HostedState;
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use std::sync::LazyLock;

/// Covers both the vendored community-edition migrations and this repo's own (enterprise-only)
/// ones, for use with `#[sqlx::test(migrator = "test_helpers::COMBINED_MIGRATOR")]`.
#[allow(dead_code)] // Not every test binary that includes this module uses it.
pub static COMBINED_MIGRATOR: LazyLock<Migrator> = LazyLock::new(|| {
    let vendor = sqlx::migrate!("../../../migrations");
    let enterprise = sqlx::migrate!("./migrations");
    let migrations = vendor
        .iter()
        .chain(enterprise.iter())
        .cloned()
        .collect::<Vec<_>>();
    Migrator {
        migrations: migrations.into(),
        ..vendor
    }
});

/// A `HostedState` with default (unconfigured) Stripe config and OAuth disabled -- the baseline
/// every test that only cares about a different field (or configures OAuth itself) starts from.
#[allow(dead_code)] // Not every test binary that includes this module uses it.
pub fn hosted_state(pool: PgPool) -> HostedState {
    HostedState {
        tenant_db: yorishiro_core::db::TenantDb::new(pool.clone()),
        identity_pool: pool,
        stripe_config: StripeConfig::default(),
        oauth_config: None,
    }
}

/// Seeds a tenant, returning its id.
#[allow(dead_code)]
pub async fn seed_tenant(pool: &PgPool, name: &str) -> uuid::Uuid {
    yorishiro_core::repositories::tenancy::create_tenant(pool, name, None)
        .await
        .unwrap()
        .id
}

/// Seeds a workspace under an existing tenant, returning its id.
#[allow(dead_code)]
pub async fn seed_workspace(pool: &PgPool, tenant_id: uuid::Uuid, name: &str) -> uuid::Uuid {
    yorishiro_core::repositories::tenancy::create_workspace(pool, tenant_id, name, None, None, None)
        .await
        .unwrap()
        .id
}

/// Seeds a tenant plus one workspace under it, returning `(tenant_id, workspace_id)`.
#[allow(dead_code)]
pub async fn seed_tenant_and_workspace(pool: &PgPool) -> (uuid::Uuid, uuid::Uuid) {
    let tenant_id = seed_tenant(pool, "test-tenant").await;
    let workspace_id = seed_workspace(pool, tenant_id, "test-workspace").await;
    (tenant_id, workspace_id)
}
