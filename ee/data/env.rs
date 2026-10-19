//! Direct environment keys owned by the enterprise overlay.

use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Env {
    Bind,
    LicenceKey,
    OAuthIssuerUrl,
    OAuthClientId,
    OAuthClientSecret,
    OAuthRedirectUri,
    StripePricePro,
    StripePriceTeam,
    StripeWebhookSecret,
}

impl Env {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Bind => "YORISHIRO_BIND",
            Self::LicenceKey => "YORISHIRO_LICENSE_KEY",
            Self::OAuthIssuerUrl => "YORISHIRO_OAUTH_ISSUER_URL",
            Self::OAuthClientId => "YORISHIRO_OAUTH_CLIENT_ID",
            Self::OAuthClientSecret => "YORISHIRO_OAUTH_CLIENT_SECRET",
            Self::OAuthRedirectUri => "YORISHIRO_OAUTH_REDIRECT_URI",
            Self::StripePricePro => "YORISHIRO_STRIPE_PRICE_PRO",
            Self::StripePriceTeam => "YORISHIRO_STRIPE_PRICE_TEAM",
            Self::StripeWebhookSecret => "YORISHIRO_STRIPE_WEBHOOK_SECRET",
        }
    }

    pub(crate) fn read(self) -> Result<String, std::env::VarError> {
        std::env::var(self.name())
    }

    pub(crate) fn read_os(self) -> Option<OsString> {
        std::env::var_os(self.name())
    }

    pub(crate) fn non_empty(self) -> Option<String> {
        self.read().ok().filter(|value| !value.is_empty())
    }
}
