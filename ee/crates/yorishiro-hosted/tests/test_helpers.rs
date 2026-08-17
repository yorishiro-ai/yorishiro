//! Shared test-only helpers.
//!
//! One directory holds the whole schema, so every test names it directly.

use crate::http::controllers::stripe::StripeConfig;
use crate::services::licence::{LicenceClaims, LicenceState};
use crate::state::HostedState;
use sqlx::PgPool;

/// A `HostedState` with default (unconfigured) Stripe config and OAuth disabled -- the baseline
/// every test that only cares about a different field (or configures OAuth itself) starts from.
/// Licensed by default: the tests here exercise the paid features, so an unlicensed baseline
/// would make every one of them assert a 404 about licensing instead of the behaviour it is
/// about. Tests that care about the gate itself use [`unlicensed_hosted_state`].
#[allow(dead_code)] // Not every test binary that includes this module uses it.
pub fn hosted_state(pool: PgPool) -> HostedState {
    HostedState {
        tenant_db: yorishiro_core::db::TenantDb::new(pool.clone()),
        identity_pool: pool,
        stripe_config: StripeConfig::default(),
        oauth_config: None,
        licence: test_licence(),
    }
}

/// A far-future licence, so a suite run never starts failing because a fixed expiry went past.
#[allow(dead_code)]
pub fn test_licence() -> LicenceState {
    LicenceState::licensed(LicenceClaims {
        sub: "test-suite".into(),
        plan: "enterprise".into(),
        exp: chrono::Utc::now().timestamp() + 60 * 60 * 24 * 365,
    })
}

/// The same state with no licence, for the tests that assert a gated endpoint is closed.
#[allow(dead_code)]
pub fn unlicensed_hosted_state(pool: PgPool) -> HostedState {
    HostedState {
        licence: LicenceState::default(),
        ..hosted_state(pool)
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
