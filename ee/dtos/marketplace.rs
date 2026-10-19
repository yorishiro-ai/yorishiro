use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `status` and `visibility` stay strings on the wire so an unknown value reaches the model's
/// `parse_publish` / `parse_input` and is refused with the API's own validation envelope and hint,
/// which a serde rejection would replace with a plain-text body.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct PublishVersionRequest {
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub(crate) definition: serde_json::Value,
    pub(crate) changelog: Option<String>,
    #[serde(default = "default_publish_status")]
    #[cfg_attr(
        feature = "openapi",
        schema(required = false, pattern = "^(draft|pre|stable)$", default = "draft")
    )]
    pub(crate) status: String,
}

fn default_publish_status() -> String {
    "draft".to_string()
}

#[derive(Debug, Deserialize)]
pub(crate) struct ForkParams {
    pub(crate) version: Option<i32>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct ForkResponse {
    pub(crate) template_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct SetVisibilityRequest {
    #[cfg_attr(feature = "openapi", schema(value_type = crate::models::template_templates::TemplateVisibility))]
    pub(crate) visibility: String,
}
