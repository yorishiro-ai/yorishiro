//! The entry points that serve HTTP: REST handlers, the MCP server, request middleware and extractors.
//! Each handler authenticates, parses input, calls a model operation and renders the result.

// REST resources: one module per resource, matching `dtos/`.
pub(crate) mod api_keys;
pub mod audit_log;
pub mod auth;
pub mod entities;
pub mod export;
pub(crate) mod iam;
pub mod import;
pub mod members;
pub mod relations;
pub mod schemas;
pub mod search;
pub(crate) mod setup;
pub mod template_library;
pub(crate) mod tenants;
pub(crate) mod users;
pub(crate) mod whoami;
pub mod workspaces;

// MCP tools, served beside REST by the same models.
pub mod mcp;

// Shared by every handler: error rendering, extractors, middleware and the OpenAPI document.
pub mod error;
pub mod extractors;
pub mod middleware;
pub mod openapi;

pub use error::ApiError;
