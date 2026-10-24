//! Stripe credentials and price mappings from resolved Loco configuration.

use loco_rs::config::Config;
use serde::Deserialize;

use super::plan::StripePriceMapping;

#[derive(Clone, Default)]
pub(crate) struct StripeConfig {
    pub(crate) webhook_secret: Option<String>,
    pub(crate) price_mapping: StripePriceMapping,
}

impl StripeConfig {
    pub(crate) fn from_config(config: &Config) -> loco_rs::Result<Self> {
        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Settings {
            webhook_secret: Option<String>,
            price_pro: Option<String>,
            price_team: Option<String>,
        }
        let settings: Settings = config
            .initializers
            .as_ref()
            .and_then(|values| values.get("stripe"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            webhook_secret: settings.webhook_secret.filter(|value| !value.is_empty()),
            price_mapping: StripePriceMapping {
                pro_price_id: settings.price_pro.filter(|value| !value.is_empty()),
                team_price_id: settings.price_team.filter(|value| !value.is_empty()),
            },
        })
    }
}
