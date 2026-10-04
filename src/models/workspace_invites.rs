pub use crate::models::_entities::workspace_invites::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

use crate::models::tenant_memberships::MembershipRole;

mod operations;

pub use operations::create_invite;
pub(crate) use operations::redeem_invite;

pub struct RedeemedInvite {
    pub tenant_id: Uuid,
    pub email: String,
    pub role: MembershipRole,
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod test_support {
    pub use super::operations::test_support::{create_invite_at, redeem_invite_at};
}

#[async_trait::async_trait]
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

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}
