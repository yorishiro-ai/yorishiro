use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SetEmbeddingKeyRequest {
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// Stored as given and never returned.
    pub(crate) api_key: String,
    pub(crate) dimensions: i32,
    #[serde(default)]
    pub(crate) send_dimensions_param: bool,
}
