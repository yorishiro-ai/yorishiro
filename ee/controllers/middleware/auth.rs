//! Authentication adapters for enterprise routes.
//!
//! Key resolution itself is edition-neutral and lives on the base `AuthContext` extractor, so a tenant-scoped key works identically on community and enterprise routes.

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::AuthContext as Authenticated;
use crate::models::api_keys::AuthContext;

/// The header naming which workspace a tenant-scoped API key should act on.
pub use crate::models::api_keys::WORKSPACE_HEADER;

/// The caller **with its workspace**: a workspace-scoped key names its own, a tenant-scoped one names it per request with `X-Workspace-Id`.
///
/// Use this whenever the work touches a workspace's own content, since that is what the RLS-scoped connection has to be opened against.
pub struct WorkspaceAuthenticated(pub AuthContext);

impl<S> FromRequestParts<S> for WorkspaceAuthenticated
where
    AppContext: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Authenticated(auth) = Authenticated::from_request_parts(parts, state).await?;
        auth.require_workspace()?;
        Ok(Self(auth))
    }
}

impl aide::OperationInput for WorkspaceAuthenticated {
    fn operation_input(
        ctx: &mut aide::generate::GenContext,
        operation: &mut aide::openapi::Operation,
    ) {
        <Authenticated as aide::OperationInput>::operation_input(ctx, operation);
    }
}
