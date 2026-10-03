use serde::Deserialize;

#[derive(Deserialize)]
pub struct SearchEntitiesParams {
    pub query_text: String,
    pub entity_type: Option<String>,
    /// JSON-encoded containment filter, e.g. `{"status":"active"}`.
    pub filter: Option<String>,
    pub limit: Option<i64>,
}
