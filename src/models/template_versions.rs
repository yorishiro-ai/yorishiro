//! The `template_versions` table.
//! Only the overlay marketplace reads and writes it, but every generated entity needs its own module so its `ActiveModelBehavior` is implemented in every build.

pub use crate::models::_entities::template_versions::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

impl ActiveModelBehavior for ActiveModel {}
