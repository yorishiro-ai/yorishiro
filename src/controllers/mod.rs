//! The entry points that serve HTTP: REST handlers, the MCP server, request middleware and extractors.
//! Each handler authenticates, parses input, calls a model operation and renders the result.

// REST resources: one module per resource, matching `dtos/`.
pub(crate) mod access_keys;
pub mod auth;
pub(crate) mod caller_identity;
pub mod entities;
pub mod export;
pub(crate) mod groups;
pub(crate) mod iam;
pub mod import;
pub(crate) mod policies;
pub mod relations;
pub mod schemas;
pub mod search;
pub(crate) mod setup;
pub mod template_library;
pub(crate) mod tenants;
pub(crate) mod users;
pub mod workspaces;

// MCP tools, served beside REST by the same models.
pub mod mcp;

// Shared by every handler: error rendering, extractors, middleware and the OpenAPI document.
pub mod error;
pub mod extractors;
pub mod middleware;
pub mod openapi;

pub use error::{Accepted, ApiError, Created};
fn action_resource_id(
    segment: &str,
    suffix: &'static str,
) -> Result<uuid::Uuid, crate::YorishiroError> {
    segment
        .strip_suffix(suffix)
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .ok_or_else(|| crate::YorishiroError::not_found("route was not found"))
}
