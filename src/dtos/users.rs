use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub created_at: String,
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
            created_at: crate::time::in_timezone(user.created_at, crate::time::configured(ctx))
                .to_rfc3339(),
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateUserRequest {
    /// New display name.
    /// Null clears it.
    /// Email, password and OAuth subjects are not editable here.
    pub display_name: Option<String>,
}
