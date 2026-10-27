pub use crate::models::_entities::workspace_invites::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

use crate::models::tenant_memberships::MembershipRole;

mod operations;

pub(crate) use operations::{accept_authenticated_user, accept_existing_user, register_user};
pub use operations::{create_invite, create_invite_at, redeem_invite_at};

pub struct RedeemedInvite {
    pub tenant_id: Uuid,
    pub email: String,
    pub role: MembershipRole,
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
