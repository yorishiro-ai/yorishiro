//! Reading which schemas have an origin template that moved on without them.
//!
//! The query alone: what to do about a change is `models::schema_schemas`'s, which decides whether a merge is safe and what it would produce.
//!
//! Owns no table.
//! `schema_schemas` and `template_templates` are both base's; both are read here on `ctx.db` (the migration/admin connection), since `template_templates` carries no GRANT to `yorishiro_app` and a request's RLS-scoped connection cannot see it at all.
//! That does not make this base's: the endpoint it serves is enterprise, and an edition is decided by what a feature is rather than by which tables it reads.

use super::merge;
use crate::error::{ResultExt, YorishiroError};
use crate::models::pagination::ListParams;
use crate::models::schema_schemas::UpstreamChange;
use crate::models::schema_schemas::metaschema::MetaSchemaDefinition;
use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, JoinType, QueryFilter, QueryOrder,
    QuerySelect,
};
use uuid::Uuid;

#[derive(FromQueryResult)]
struct Row {
    schema_id: Uuid,
    schema_name: String,
    version: i32,
    template_id: Uuid,
    template_name: String,
    changed_at: DateTime<Utc>,
    origin_updated_at: Option<DateTime<Utc>>,
    schema_definition: sea_orm::prelude::Json,
    origin_snapshot: Option<sea_orm::prelude::Json>,
    template_definition: sea_orm::prelude::Json,
}

/// Schemas in this workspace whose origin template has changed since the copy was taken.
///
/// Nothing is applied.
/// The upstream edit does not reach the copy on its own (an automatic update could make stored entities invalid against a definition nobody here chose), so this reports and the workspace decides.
///
/// A schema whose template was deleted is not reported: the trigger has already detached it, and there is no longer an update to take.
/// `linked` is the whole population here.
///
/// The explicit `origin_updated_at IS NULL` state is the source of truth.
/// It survives local schema versioning and is cleared only by a successful merge acknowledgement.
/// Reads both definitions in one joined ORM query before computing the merge summary.
pub(crate) async fn list_with_upstream_changes(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    page: ListParams,
) -> Result<Vec<UpstreamChange>, YorishiroError> {
    use crate::models::_entities::{schema_schemas as schema, template_templates as template};
    let rows = schema::Entity::find()
        .select_only()
        .column_as(schema::Column::Id, "schema_id")
        .column_as(schema::Column::Name, "schema_name")
        .column(schema::Column::Version)
        .column_as(template::Column::Id, "template_id")
        .column_as(template::Column::Name, "template_name")
        .column_as(template::Column::UpdatedAt, "changed_at")
        .column(schema::Column::OriginUpdatedAt)
        .column_as(schema::Column::Definition, "schema_definition")
        .column(schema::Column::OriginSnapshot)
        .column_as(template::Column::Definition, "template_definition")
        .join(
            JoinType::InnerJoin,
            schema::Entity::belongs_to(template::Entity)
                .from(schema::Column::OriginTemplateId)
                .to(template::Column::Id)
                .into(),
        )
        .filter(schema::Column::WorkspaceId.eq(workspace_id))
        .filter(schema::Column::Status.eq("active"))
        .filter(schema::Column::OriginStatus.eq("linked"))
        .filter(schema::Column::OriginUpdatedAt.is_null())
        .order_by_desc(template::Column::UpdatedAt)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .into_model::<Row>()
        .all(conn)
        .await
        .internal()?;

    rows.into_iter()
        .map(|row| {
            let schema_definition: MetaSchemaDefinition =
                serde_json::from_value(row.schema_definition)
                    .map_err(|err| YorishiroError::Internal(err.into()))?;
            let base: MetaSchemaDefinition = row
                .origin_snapshot
                .ok_or_else(|| {
                    YorishiroError::Internal(anyhow::anyhow!("linked schema has no merge base"))
                })
                .and_then(|value| {
                    serde_json::from_value(value)
                        .map_err(|err| YorishiroError::Internal(err.into()))
                })?;
            let template_definition: MetaSchemaDefinition =
                serde_json::from_value(row.template_definition)
                    .map_err(|err| YorishiroError::Internal(err.into()))?;
            let summary = merge::three_way(&base, &template_definition, &schema_definition).summary;
            Ok(UpstreamChange {
                schema_id: row.schema_id,
                schema_name: row.schema_name,
                version: row.version,
                template_id: row.template_id,
                template_name: row.template_name,
                changed_at: row.changed_at,
                pending_notification: row.origin_updated_at.is_none(),
                summary,
            })
        })
        .collect::<Result<Vec<_>, YorishiroError>>()
}
