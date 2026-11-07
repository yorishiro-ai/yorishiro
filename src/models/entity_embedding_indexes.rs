//! Persisted, disposable HNSW snapshots; original vectors remain authoritative.

use sea_orm::entity::prelude::*;

use super::_entities::entity_embedding_indexes::ActiveModel;

impl ActiveModelBehavior for ActiveModel {}
