use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct UserRecord {
    pub(crate) id: Uuid,
    pub(crate) email: String,
    pub(crate) display_name: Option<String>,
    pub(crate) created_at: String,
}

impl UserRecord {
    pub(crate) fn render(
        ctx: &loco_rs::app::AppContext,
        user: crate::models::user_users::Model,
    ) -> Self {
        Self {
            id: user.id,
            email: user.email,
            display_name: user.display_name,
            created_at: crate::time::render(ctx, user.created_at).to_rfc3339(),
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct UpdateUserRequest {
    /// New display name.
    /// Null clears it.
    /// Email, password and OAuth subjects are not editable here.
    pub(crate) display_name: Option<String>,
}
