pub mod audit_log;
pub mod auth;
pub mod entities;
pub mod error;
pub mod export;
pub mod extractors;
pub mod import;
pub mod mcp;
pub mod members;
pub mod relations;
pub mod schemas;
pub mod search;
pub mod setup;
pub mod system;
pub mod template_library;
pub mod whoami;
pub mod workspaces;

pub use error::ApiError;

use crate::error::YorishiroError;

/// The `limit`/`offset` query-string pair every list endpoint accepts.
/// `#[serde(flatten)]` this into a request's own `Params` struct alongside its filters, the same
/// way `models::pagination::ListParams` is embedded into a table's own `ListXQuery`.
#[derive(Default, serde::Deserialize)]
pub struct PageParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl From<PageParams> for crate::models::pagination::ListParams {
    fn from(params: PageParams) -> Self {
        Self::new(params.limit, params.offset)
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
