pub use crate::models::_entities::user_users::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

mod operations;

pub use operations::DeletionOutcome;
pub use operations::create_user;
pub use operations::delete_by_operator;
pub use operations::reset_root_password_by_operator;
pub(crate) use operations::{create_tenant_user, reset_tenant_password};
pub(crate) use operations::{find_in_tenant, update_display_name};
pub(crate) use operations::{verify_login, verify_tenant_login};

impl Model {
    /// The YRN of the user in `tenant_id`; a root user has no name and uses the tenant's root YRN.
    pub(crate) fn yrn(&self, tenant_id: Uuid) -> String {
        self.user_name.as_deref().map_or_else(
            || crate::models::iam::names::root_yrn(tenant_id),
            |name| crate::models::iam::names::user_yrn(tenant_id, name),
        )
    }

    /// The email of a root user, which every root user has.
    pub(crate) fn root_email(&self) -> String {
        self.email.clone().unwrap_or_default()
    }
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
