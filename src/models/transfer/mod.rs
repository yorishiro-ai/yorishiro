//! The shared JSON Lines format for workspace export and import.

use serde::{Deserialize, Serialize};

use super::entity_entities::EntityRecord;
use super::entity_relations::RelationRecord;
use super::schema_schemas::SchemaRecord;

pub mod export;
pub mod import;

/// One JSONL record, tagged so import can distinguish schemas, entities and relations.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub(crate) enum ExportRecord {
    Schema(SchemaRecord),
    Entity(EntityRecord),
    Relation(RelationRecord),
}
