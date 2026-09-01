use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetaSchemaDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub entity_types: BTreeMap<String, EntityTypeDef>,
    #[serde(default)]
    pub relation_types: BTreeMap<String, RelationTypeDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityTypeDef {
    #[serde(default)]
    pub description: Option<String>,
    pub fields: BTreeMap<String, FieldDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationTypeDef {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldTypeName {
    String,
    Number,
    Integer,
    Boolean,
    Array,
    Object,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrayItems {
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, FieldDef>>,
}

/// Field definition using standard JSON Schema keywords.
/// Unknown `x-` attributes are preserved via `extra` (flattened) so that older clients don't drop fields they don't recognize yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub r#type: FieldTypeName,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(default, rename = "minLength", skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    #[serde(default, rename = "maxLength", skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    #[serde(default, rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
    #[serde(
        default,
        rename = "uniqueItems",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub unique_items: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<ArrayItems>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, FieldDef>>,
    #[serde(
        default,
        rename = "x-embed",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub x_embed: bool,
    #[serde(default, rename = "x-ui", skip_serializing_if = "Option::is_none")]
    pub x_ui: Option<Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}
