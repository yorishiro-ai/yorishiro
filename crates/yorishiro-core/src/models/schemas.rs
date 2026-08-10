use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::metaschema::MetaSchemaDefinition;

/// Represents a row in the `schemas` table. `definition` is JSONB in the DB, but the
/// application layer always treats it as a parsed `MetaSchemaDefinition`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SchemaRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    /// The workspace that owns this schema. Schemas are per workspace: applying a template
    /// gives a workspace its own copy, so a sibling workspace's edits do not reach it.
    ///
    /// `default` on deserialize because this type is also the JSONL export record, and an
    /// export taken before schemas became workspace-scoped carries no such field. Import
    /// assigns the destination workspace regardless — it remaps every id it reads — so the
    /// value in the file is never the one that lands.
    #[serde(default)]
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    pub definition: MetaSchemaDefinition,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// A row in a schema listing. A lightweight summary that omits the `definition` body,
/// used as the entry point for MCP clients (LLMs) to discover what schemas exist for a
/// tenant.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
pub struct SchemaSummary {
    pub id: Uuid,
    pub name: String,
    pub version: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
#[path = "../../tests/models/schemas.rs"]
mod tests;
