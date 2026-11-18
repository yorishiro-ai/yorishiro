pub use crate::models::_entities::tenant_memberships::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
use serde::Serialize;

mod operations;

pub(crate) use operations::create_tenant_user_as;
pub use operations::preview_removal;
pub use operations::{add_member, remove_member, remove_member_as};
pub(crate) use operations::{add_member_in_transaction, is_member, list_members, list_members_as};
pub use operations::{create_tenant_user_member, reset_tenant_user_password};

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: Option<String>,
    pub user_name: Option<String>,
    pub display_name: Option<String>,
}

/// A point-in-time snapshot, not authorization for a later mutation.
#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipRemovalPreview {
    pub user_id: Uuid,
    pub member: bool,
    pub revoked_api_key_ids: Vec<Uuid>,
    pub lost_workspace_ids: Vec<Uuid>,
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
