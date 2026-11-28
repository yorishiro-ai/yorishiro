//! Request and response transport types, one module per resource.

pub mod access_keys;
pub mod auth;
pub(crate) mod caller_identity;
pub mod common;
pub mod entities;
pub(crate) mod groups;
pub(crate) mod iam;
pub(crate) mod jobs;
pub(crate) mod policies;
pub mod relations;
pub(crate) mod schemas;
pub(crate) mod search;
pub mod setup;
pub(crate) mod template_library;
pub(crate) mod tenants;
pub(crate) mod users;
pub mod workspaces;
