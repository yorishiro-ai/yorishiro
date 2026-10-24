use serde::Deserialize;

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct SetColumnsRequest {
    /// Field names in display order. An empty list is distinct from no preference.
    pub(crate) columns: Vec<String>,
}
