use std::sync::Arc;

use async_trait::async_trait;

use crate::db::DbHandle;
use crate::error::YorishiroError;

use super::{AuthContext, authenticate};

/// Resolves a presented API key into an [`AuthContext`].
///
/// A seam: a deployment can replace this rule (a key that names its workspace per request, a key issued by an external identity system) without touching the routes that call it.
/// [`DefaultAuthenticator`] is the behaviour of every deployment that does not replace it.
///
/// # Contract
///
/// - **must** reject a key it cannot verify, by returning [`YorishiroError::Unauthenticated`].
/// - **must** return a context whose `tenant_id` owns its `workspace_id`.
///   The RLS session variables are set from both, so a mismatched pair silently produces a session that can see one tenant's workspace under another tenant's policies.
/// - **may** read `headers` for anything the key itself does not carry.
#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn authenticate(
        &self,
        db: &DbHandle,
        presented_key: &str,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError>;
}

/// This crate's own rule: a key is bound to exactly one workspace, recorded on the key itself, and the request's headers do not affect which one it resolves to.
pub struct DefaultAuthenticator;

#[async_trait]
impl Authenticator for DefaultAuthenticator {
    async fn authenticate(
        &self,
        db: &DbHandle,
        presented_key: &str,
        _headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        authenticate(db, presented_key).await
    }
}

/// The authenticator a deployment gets when it does not choose one.
pub fn default_authenticator() -> Arc<dyn Authenticator> {
    Arc::new(DefaultAuthenticator)
}
