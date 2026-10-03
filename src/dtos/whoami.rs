use crate::models::api_keys::ApiKeyScope;
use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WhoAmIResponse {
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    /// The user this key was issued for, if it was created with `admin create-api-key --user`.
    pub user_id: Option<Uuid>,
    /// Independent of `scope`: whether this key additionally holds the audit grant, checked separately by `GET /api/audit-log`.
    pub audit: bool,
}
