use sqlx::PgPool;
use yorishiro_core::db::TenantDb;

use crate::http::controllers::stripe::StripeConfig;
use crate::services::licence::LicenceState;
use crate::services::oauth::OAuthConfig;

/// Shared state for the hosted admin dashboard/Stripe webhook/OAuth router.
/// `identity_pool` connects as the admin/migration role (same as `yorishiro-server`'s `AppState::identity_pool`), since every operation here (billing, usage aggregation across a tenant's workspaces, member listing, OAuth user provisioning) is a control-plane concern that predates or spans RLS's per-workspace scoping.
#[derive(Clone)]
pub struct HostedState {
    pub identity_pool: PgPool,
    /// The RLS-scoped pool, for the routes here that read or write tenant *content* rather than control-plane records: a workspace's own schema fork, for instance.
    /// `identity_pool` bypasses RLS entirely and must not be used for those.
    pub tenant_db: TenantDb,
    pub stripe_config: StripeConfig,
    /// `None` when `YORISHIRO_OAUTH_ISSUER_URL` is unset: see `OAuthConfig::from_env`.
    /// Every `/auth/oauth/*` route checks this first and returns `404 Not Found` when it's `None`, so OAuth is a purely additive, opt-in feature: a deployment that never sets it behaves exactly as it did before this feature existed.
    pub oauth_config: Option<OAuthConfig>,
    /// Verified once at startup from `YORISHIRO_LICENSE_KEY`.
    /// The paid gates call `require_active()` on it per request rather than reading a flag captured at boot, so a key that expires mid-run closes the gates without a restart.
    ///
    /// An unlicensed state is a supported way to run: the free half is unaffected and the gated endpoints answer `404`, the same shape as `oauth_config` being `None`.
    pub licence: LicenceState,
}

#[cfg(test)]
#[path = "../tests/state.rs"]
mod tests;
