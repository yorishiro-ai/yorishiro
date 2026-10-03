use crate::metaschema::MetaSchemaDefinition;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub definition: MetaSchemaDefinition,
    #[serde(default)]
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub author: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateTemplateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub definition: Option<MetaSchemaDefinition>,
    pub tags: Option<Vec<String>>,
    pub locale: Option<String>,
}

#[derive(Deserialize)]
pub struct ForkTemplateRequest {
    pub name: String,
}
