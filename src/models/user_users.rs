pub use crate::models::_entities::user_users::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

mod operations;

pub use operations::create_user;
pub(crate) use operations::{create_tenant_user, reset_tenant_password};
pub(crate) use operations::{current_profile, update_profile};
pub(crate) use operations::{delete_by_operator, list_by_operator};
pub(crate) use operations::{get_user_by_email, verify_login, verify_tenant_login};

impl Model {
    /// The email of a root user, which every root user has.
    pub(crate) fn root_email(&self) -> String {
        self.email.clone().unwrap_or_default()
    }
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
