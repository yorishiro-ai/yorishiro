use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct SetColumnsRequest {
    /// Field names in display order. An empty list is distinct from no preference.
    pub(crate) columns: Vec<String>,
}
