use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct MarketplaceVersionPath {
    pub(crate) template_id: Uuid,
    pub(crate) version_id: Uuid,
}

/// `status` and `visibility` stay strings on the wire so an unknown value reaches the model's
/// `parse_publish` / `parse_input` and is refused with the API's own validation envelope and hint,
/// which a serde rejection would replace with a plain-text body.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct PublishVersionRequest {
    #[schemars(with = "serde_json::Value")]
    pub(crate) definition: serde_json::Value,
    pub(crate) changelog: Option<String>,
    #[serde(default = "default_publish_status")]
    #[schemars(with = "crate::edition::ee::models::marketplace::TemplateVersionStatus")]
    pub(crate) status: String,
}

fn default_publish_status() -> String {
    "draft".to_string()
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ForkParams {
    pub(crate) version: Option<i32>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct ForkResponse {
    pub(crate) template_id: Uuid,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SetVisibilityRequest {
    #[schemars(with = "crate::models::template_templates::TemplateVisibility")]
    pub(crate) visibility: String,
}
