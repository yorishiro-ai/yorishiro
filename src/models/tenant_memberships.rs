pub use crate::models::_entities::tenant_memberships::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
use serde::Serialize;

use crate::db_enum::db_enum;
use crate::models::api_keys::ApiKeyScope;

mod operations;

pub use operations::preview_removal;
pub use operations::preview_role_change;
pub use operations::transfer_ownership;
pub use operations::{add_member, remove_member, remove_member_as, update_member_access};
pub(crate) use operations::{add_member_in_transaction, get_membership_role, list_members};

db_enum! {
    /// Mirrors the `tenant_memberships.role` check constraint.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum MembershipRole {
        Owner = "owner",
        Admin = "admin",
        Member = "member",
        Viewer = "viewer",
    }
}

impl MembershipRole {
    pub fn max_scope(self) -> ApiKeyScope {
        match self {
            Self::Owner | Self::Admin => ApiKeyScope::Migration,
            Self::Member => ApiKeyScope::Write,
            Self::Viewer => ApiKeyScope::Read,
        }
    }

    pub(crate) fn administers_tenant(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub role: MembershipRole,
}

/// Key access observed while updating a membership, excluding secret material.
#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipKeyAccess {
    pub api_key_id: Uuid,
    pub stored_scope: ApiKeyScope,
    pub effective_scope: Option<ApiKeyScope>,
    /// Explicit operator grant, independent of the membership's ordered scope cap.
    pub audit: bool,
}

/// A point-in-time snapshot, not authorization for a later mutation.
#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipRemovalPreview {
    pub user_id: Uuid,
    pub role: Option<MembershipRole>,
    pub blocked_by_last_owner: bool,
    pub revoked_api_key_ids: Vec<Uuid>,
    pub lost_workspace_ids: Vec<Uuid>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipRolePreview {
    pub user_id: Uuid,
    pub current_role: MembershipRole,
    pub requested_role: MembershipRole,
    pub blocked_by_last_owner: bool,
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
