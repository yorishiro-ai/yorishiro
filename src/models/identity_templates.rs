pub use super::_entities::identity_templates::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
pub type IdentityTemplates = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// Checks `!is_set()` rather than `is_unchanged()`: an `ActiveModel` built with
    /// `..Default::default()` leaves untouched fields `NotSet`, not `Unchanged`, and
    /// `is_unchanged()` only matches the latter. See `content_entities.rs`'s copy of this
    /// comment for where this was caught live.
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && !self.updated_at.is_set() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}
