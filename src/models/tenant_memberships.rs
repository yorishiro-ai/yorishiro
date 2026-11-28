pub use crate::models::_entities::tenant_memberships::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
use serde::Serialize;

mod operations;

pub use operations::create_tenant_user_member;
pub use operations::list_members;
pub use operations::{add_member, remove_member};
pub(crate) use operations::{
    add_member_in_transaction, is_member, list_member_users, remove_member_in_transaction,
};

#[derive(Serialize, schemars::JsonSchema)]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: Option<String>,
    pub user_name: Option<String>,
    pub display_name: Option<String>,
}

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        Ok(this)
    }
}
