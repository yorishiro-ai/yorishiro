//! The `stripe_events` table.
//! Only the overlay billing code reads and writes it, but every generated entity needs its own module so its `ActiveModelBehavior` is implemented in every build.

pub use crate::models::_entities::stripe_events::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;

impl ActiveModelBehavior for ActiveModel {}
