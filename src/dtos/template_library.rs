use crate::models::schema_schemas::metaschema::MetaSchemaDefinition;
use serde::Deserialize;

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub(crate) struct CreateTemplateRequest {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    #[cfg_attr(feature = "openapi", schemars(with = "serde_json::Value"))]
    pub(crate) definition: MetaSchemaDefinition,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    pub(crate) locale: Option<String>,
    pub(crate) author: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub(crate) struct UpdateTemplateRequest {
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    #[cfg_attr(feature = "openapi", schemars(with = "Option<serde_json::Value>"))]
    pub(crate) definition: Option<MetaSchemaDefinition>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) locale: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub(crate) struct ForkTemplateRequest {
    pub(crate) name: String,
}
