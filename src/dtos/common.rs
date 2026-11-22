//! Request types shared by every list endpoint.

use crate::error::YorishiroError;

/// The `page`/`page_size` query-string pair every list endpoint accepts.
/// `#[serde(flatten)]` this into a request's own `Params` struct alongside its filters.
///
/// Wraps Loco's `query::PaginationQuery` (1-based page + `page_size`).  Loco's own
/// query-string deserializer maps `?page=&page_size=` into the struct fields, so
/// all existing API callers work without changes.  The `TryFrom` impl converts to the
/// internal 0-based offset/limit representation that every list function expects.
#[derive(Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
#[schemars(inline)]
pub struct PageParams(
    #[schemars(with = "PaginationSchema")] pub loco_rs::model::query::PaginationQuery,
);

impl TryFrom<PageParams> for crate::models::pagination::ListParams {
    type Error = YorishiroError;

    fn try_from(params: PageParams) -> Result<Self, Self::Error> {
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

#[derive(schemars::JsonSchema)]
#[schemars(inline)]
#[expect(
    dead_code,
    reason = "Schema projection for Loco's external pagination type"
)]
struct PaginationSchema {
    page: Option<u64>,
    page_size: Option<u64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct EntityTypePath<T0> {
    pub entity_type: T0,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ForkIdPath<T0> {
    pub fork_id: T0,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct GroupIdPath<T0> {
    pub group_id: T0,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct GroupIdPolicyIdPath<T0, T1> {
    pub group_id: T0,
    pub policy_id: T1,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct GroupIdUserIdPath<T0, T1> {
    pub group_id: T0,
    pub user_id: T1,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct IdPath<T0> {
    pub id: T0,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PolicyIdPath<T> {
    pub policy_id: T,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct IdUserIdPath<T0, T1> {
    pub id: T0,
    pub user_id: T1,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct JobIdPath<T0> {
    pub job_id: T0,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NameEntityTypePath<T0, T1> {
    pub name: T0,
    pub entity_type: T1,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NamePath<T0> {
    pub name: T0,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SchemaIdPath<T0> {
    pub schema_id: T0,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UserIdPath<T0> {
    pub user_id: T0,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UserIdPolicyIdPath<T0, T1> {
    pub user_id: T0,
    pub policy_id: T1,
}
