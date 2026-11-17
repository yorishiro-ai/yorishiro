pub use crate::models::_entities::tenant_memberships::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
use serde::Serialize;

use crate::db_enum::db_enum;
use crate::models::api_keys::ApiKeyScope;

mod operations;

pub(crate) use operations::create_tenant_user_as;
pub use operations::preview_removal;
pub use operations::preview_role_change;
pub use operations::transfer_ownership;
pub use operations::{add_member, remove_member, remove_member_as, update_member_access};
pub(crate) use operations::{
    add_member_in_transaction, get_membership_role, list_members, list_members_as,
    update_member_by_email_as,
};
pub use operations::{create_tenant_user_member, reset_tenant_user_password};

db_enum! {
    /// Display label for a tenant membership; permissions come from IAM policies, not from this value.
    /// `from_db_str` and `FromStr` also accept the retired `"owner"` spelling, which the data migration rewrote to `"admin"`.
    #[derive(Clone, Copy, PartialEq, Eq, schemars::JsonSchema)]
    #[schemars(rename_all = "lowercase")]
    pub enum MembershipRole {
        Admin = "admin" | "owner",
        Member = "member",
        Viewer = "viewer",
    }
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: Option<String>,
    pub user_name: Option<String>,
    pub display_name: Option<String>,
    pub role: MembershipRole,
}

/// A point-in-time snapshot of the member's live credentials, excluding secret material.
#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipKeyAccess {
    pub api_key_id: Uuid,
    pub stored_scope: ApiKeyScope,
    /// The credential's own scope: membership labels no longer lower it, and IAM policies gate individual actions rather than scopes.
    pub effective_scope: Option<ApiKeyScope>,
    /// Explicit operator grant, independent of the credential's scope.
    pub audit: bool,
}

/// A point-in-time snapshot, not authorization for a later mutation.
#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipRemovalPreview {
    pub user_id: Uuid,
    pub role: Option<MembershipRole>,
    /// Set when this removal leaves no non-root admin; only the tenant root retains full access and can restore administrator membership.
    pub last_admin_warning: Option<String>,
    pub revoked_api_key_ids: Vec<Uuid>,
    pub lost_workspace_ids: Vec<Uuid>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipRolePreview {
    pub user_id: Uuid,
    pub current_role: MembershipRole,
    pub requested_role: MembershipRole,
    /// Set when this change leaves no non-root admin; only the tenant root retains full access and can restore administrator membership.
    pub last_admin_warning: Option<String>,
    pub lost_workspace_ids: Vec<Uuid>,
    pub keys: Vec<MembershipKeyAccess>,
}

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// `id` has a `uuidv7()` column default on PostgreSQL and no default on SQLite; see `crate::db::sqlite_generated_id`.
    async fn before_save<C>(mut self, db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
    }
}
