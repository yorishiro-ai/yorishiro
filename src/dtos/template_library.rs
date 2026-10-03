use crate::models::schema_schemas::metaschema::MetaSchemaDefinition;
use serde::Deserialize;

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    #[cfg_attr(feature = "openapi", schema(value_type = serde_json::Value))]
    pub definition: MetaSchemaDefinition,
    #[serde(default)]
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub author: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateTemplateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<serde_json::Value>))]
    pub definition: Option<MetaSchemaDefinition>,
    pub tags: Option<Vec<String>>,
    pub locale: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ForkTemplateRequest {
    pub name: String,
}
