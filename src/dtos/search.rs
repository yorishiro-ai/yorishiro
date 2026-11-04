use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SearchEntitiesParams {
    pub(crate) query_text: String,
    pub(crate) entity_type: Option<String>,
    /// JSON-encoded containment filter, e.g. `{"status":"active"}`.
    pub(crate) filter: Option<String>,
    pub(crate) limit: Option<i64>,
}
