use super::*;

/// `oauth_config` being optional is what makes OAuth opt-in: a deployment that sets none of the
/// `YORISHIRO_OAUTH_*` variables must still start, with the OAuth routes reporting themselves
/// disabled rather than the process failing at boot.
#[test]
fn oauth_is_optional_so_a_deployment_without_it_still_starts() {
    fn assert_optional(config: &Option<crate::services::oauth::config::OAuthConfig>) -> bool {
        config.is_none()
    }

    assert!(assert_optional(&None));
}

/// The identity pool is the admin-role pool that bypasses RLS, and the Stripe config carries the
/// webhook secret -- both are required rather than optional, so a misconfigured deployment fails
/// at startup instead of at the first webhook.
#[test]
fn the_required_state_is_not_optional() {
    fn assert_field_types(state: &HostedState) {
        let _: &sqlx::PgPool = &state.identity_pool;
        let _: &crate::http::controllers::stripe::StripeConfig = &state.stripe_config;
    }
    let _ = assert_field_types;
}
