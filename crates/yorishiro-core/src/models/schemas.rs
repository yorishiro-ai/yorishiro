use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::metaschema::MetaSchemaDefinition;

/// Represents a row in the `schemas` table.
/// `definition` is JSONB in the DB, but the application layer always treats it as a parsed `MetaSchemaDefinition`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SchemaRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    /// The workspace that owns this schema.
    /// Schemas are per workspace: applying a template gives a workspace its own copy, so a sibling workspace's edits do not reach it.
    ///
    /// `default` on deserialize because this type is also the JSONL export record, and an export taken before schemas became workspace-scoped carries no such field.
    /// Import assigns the destination workspace regardless (it remaps every id it reads), so the value in the file is never the one that lands.
    #[serde(default)]
    pub workspace_id: Uuid,
    pub name: String,
    pub version: i32,
    pub definition: MetaSchemaDefinition,
    pub status: String,
    /// The template this schema was created from, when it was created from one.
    /// `None` for a schema written by hand, and `None` again once that template is deleted.
    ///
    /// `default` on deserialize for the same reason as `workspace_id`: this type doubles as the JSONL export record, and an export taken before the column existed carries no such field.
    #[serde(default)]
    pub origin_template_id: Option<Uuid>,
    /// `linked` while the origin is still there to follow, `detached` otherwise, including for every schema that never had one.
    #[serde(default = "default_origin_status")]
    pub origin_status: String,
    /// The template's definition as it stood when this copy was taken: the common ancestor a three-way merge compares against.
    ///
    /// `None` for a schema with no origin, and for one copied before this was recorded: what the template said then is not recoverable, and a fabricated base is worse than none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_snapshot: Option<MetaSchemaDefinition>,
    pub created_at: DateTime<Utc>,
}

fn default_origin_status() -> String {
    ORIGIN_STATUS_DETACHED.to_string()
}

/// Following a template that still exists.
pub const ORIGIN_STATUS_LINKED: &str = "linked";

/// Not following anything: written by hand, or following a template that has since been deleted.
/// The two are told apart by whether `origin_template_id` was ever set, which is what the notification path needs to know.
pub const ORIGIN_STATUS_DETACHED: &str = "detached";

/// A schema whose origin template has been edited since the copy was taken.
///
/// The signal only (what changed and where), with no diff and no application.
/// Whether to follow the upstream edit is the workspace's call, since applying it could invalidate entities already stored against the current definition.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UpstreamChange {
    pub schema_id: Uuid,
    pub schema_name: String,
    /// The version of the schema currently in use here.
    pub version: i32,
    pub template_id: Uuid,
    pub template_name: String,
    /// When the template was last edited.
    pub changed_at: DateTime<Utc>,
}

/// A row in a schema listing.
/// A lightweight summary that omits the `definition` body, used as the entry point for MCP clients (LLMs) to discover what schemas exist for a tenant.
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
