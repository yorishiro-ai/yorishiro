use serde::Deserialize;

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = EmbeddingKeyRequest))]
pub(crate) struct SetEmbeddingKeyRequest {
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// Stored as given and never returned.
    pub(crate) api_key: String,
    pub(crate) dimensions: i32,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(default = false))]
    pub(crate) send_dimensions_param: bool,
}
