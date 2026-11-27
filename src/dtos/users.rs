//! Request and response types for users.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, JsonSchema)]
pub(crate) struct UserRecord {
    pub(crate) user_id: Uuid,
    /// `yrn:iam:<tenant-id>:user/<name>`, or `yrn:iam:<tenant-id>:root` for the tenant root.
    pub(crate) yrn: String,
    pub(crate) tenant_id: Uuid,
    pub(crate) email: Option<String>,
    pub(crate) user_name: Option<String>,
    pub(crate) display_name: Option<String>,
    pub(crate) created_at: String,
}

impl UserRecord {
    pub(crate) fn render(
        ctx: &loco_rs::app::AppContext,
        tenant_id: Uuid,
        user: crate::models::user_users::Model,
    ) -> Self {
        Self {
            user_id: user.id,
            yrn: user.yrn(tenant_id),
            tenant_id,
            email: user.email,
            user_name: user.user_name,
            display_name: user.display_name,
            created_at: crate::data::time::render(ctx, &user.created_at).to_rfc3339(),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateUserRequest {
    /// Unique within the tenant, ignoring case: 1 to 64 characters from letters, digits and `+ = , . @ _ -`.
    pub(crate) user_name: String,
    /// At least 12 characters.
    pub(crate) password: String,
    pub(crate) display_name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateUserRequest {
    /// New display name.
    /// Null clears it.
    /// The user name, email, password and OAuth subject are not editable here.
    pub(crate) display_name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateLoginProfileRequest {
    /// At least 12 characters.
    /// Every access key issued to the user is revoked.
    pub(crate) password: String,
}

/// The tool arguments that name one user.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UserId {
    pub user_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateUserArgs {
    pub user_id: Uuid,
    /// New display name.
    /// Null clears it.
    pub display_name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateLoginProfileArgs {
    pub user_id: Uuid,
    /// At least 12 characters.
    /// Every access key issued to the user is revoked.
    pub password: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListUserAssociations {
    pub user_id: Uuid,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

crate::dtos::common::list_params_from!(ListUserAssociations);
