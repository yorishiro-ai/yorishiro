pub use crate::models::_entities::template_versions::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

impl ActiveModelBehavior for ActiveModel {}
