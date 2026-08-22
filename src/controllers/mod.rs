pub mod entities;
pub mod error;
pub mod extractors;

pub use error::ApiError;

use crate::error::YorishiroError;

/// Parses a query-string `filter` parameter (a JSON object, e.g. `{"status":"active"}`) used as
/// a JSONB containment filter. `None`/empty means no filter.
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
