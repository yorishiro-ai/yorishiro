pub use crate::models::_entities::stripe_events::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

impl ActiveModelBehavior for ActiveModel {}
