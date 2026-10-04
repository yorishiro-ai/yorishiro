pub use crate::models::_entities::tenant_memberships::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
use serde::Serialize;

use crate::db_enum::db_enum;
use crate::models::api_keys::ApiKeyScope;

mod operations;

pub use operations::add_member;
pub(crate) use operations::{get_membership_role, list_members};

db_enum! {
    /// Mirrors the `tenant_memberships.role` check constraint.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub role: MembershipRole,
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
