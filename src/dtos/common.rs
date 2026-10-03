//! Request types shared by every list endpoint.

use crate::error::YorishiroError;

/// The `page`/`page_size` query-string pair every list endpoint accepts.
/// `#[serde(flatten)]` this into a request's own `Params` struct alongside its filters.
///
/// Wraps Loco's `query::PaginationQuery` (1-based page + page_size).  Loco's own
/// query-string deserializer maps `?page=&page_size=` into the struct fields, so
/// all existing API callers work without changes.  The `From` impl converts to the
/// internal 0-based offset/limit representation that every list function expects.
#[derive(Default, serde::Deserialize)]
#[serde(transparent)]
pub struct PageParams(pub(crate) loco_rs::model::query::PaginationQuery);

impl From<PageParams> for crate::models::pagination::ListParams {
    fn from(params: PageParams) -> Self {
        Self::from_pagination_query(params.0.page, params.0.page_size)
    }
}

/// Parses a query-string `filter` parameter (a JSON object, e.g. `{"status":"active"}`) used as a JSONB containment filter.
/// `None`/empty means no filter.
pub(crate) fn parse_filter_param(
    raw: Option<String>,
) -> Result<Option<serde_json::Value>, YorishiroError> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    serde_json::from_str(&raw).map_err(|err| YorishiroError::ValidationFailed {
        message: "filter is not valid JSON".into(),
        details: vec![],
        hint: format!("filter must be a JSON object, e.g. {{\"status\":\"active\"}}: {err}"),
    })
}
