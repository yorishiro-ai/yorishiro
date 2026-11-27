use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "EmbeddingKeyRequest")]
pub(crate) struct SetEmbeddingKeyRequest {
    /// The server checks this endpoint with a sample embedding before saving.
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// Stored as given and never returned.
    pub(crate) api_key: String,
    pub(crate) dimensions: i32,
    #[serde(default)]
    pub(crate) send_dimensions_param: bool,
    #[serde(default)]
    pub(crate) query_instruction: String,
    #[serde(default)]
    pub(crate) document_prefix: String,
}
