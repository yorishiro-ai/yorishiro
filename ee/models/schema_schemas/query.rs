//! Reading which schemas have an origin template that moved on without them.
//!
//! The query alone: what to do about a change is `models::schema_schemas`'s, which decides whether a merge is safe and what it would produce.
//!
//! Owns no table.
//! The control plane joins template metadata to page the candidates.
//! Workspace schema definitions are only loaded on the RLS-scoped connection.

use super::merge;
use crate::error::{ResultExt, YorishiroError};
use crate::models::pagination::ListParams;
use crate::models::schema_schemas::metaschema::MetaSchemaDefinition;
use crate::models::schema_schemas::{SchemaOriginStatus, SchemaStatus, UpstreamChange};
use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, JoinType, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

#[derive(FromQueryResult)]
struct Row {
    workspace_id: Uuid,
    schema_id: Uuid,
    schema_name: String,
    version: i32,
    template_id: Uuid,
    template_name: String,
    changed_at: DateTime<Utc>,
    origin_updated_at: Option<DateTime<Utc>>,
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
pub(crate) async fn list_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
    page: ListParams,
) -> Result<Vec<UpstreamChange>, YorishiroError> {
    use crate::models::_entities::{schema_schemas as schema, template_templates as template};
    use crate::models::iam::{access::read_workspaces, policy::Action};
    use std::collections::HashMap;
    let workspaces = read_workspaces(&ctx.db, actor, Action::SchemaUpstreamList).await?;
    let rows = schema::Entity::find()
        .select_only()
        .column(schema::Column::WorkspaceId)
        .column_as(schema::Column::Id, "schema_id")
        .column_as(schema::Column::Name, "schema_name")
        .column(schema::Column::Version)
        .column_as(template::Column::Id, "template_id")
        .column_as(template::Column::Name, "template_name")
        .column_as(template::Column::UpdatedAt, "changed_at")
        .column(schema::Column::OriginUpdatedAt)
        .column_as(template::Column::Definition, "template_definition")
        .join(
            JoinType::InnerJoin,
            schema::Entity::belongs_to(template::Entity)
                .from(schema::Column::OriginTemplateId)
                .to(template::Column::Id)
                .into(),
        )
        .filter(schema::Column::WorkspaceId.is_in(workspaces.iter().copied()))
        .filter(schema::Column::TenantId.eq(actor.tenant_id))
        .filter(schema::Column::Status.eq(SchemaStatus::Active.as_db_str()))
        .filter(schema::Column::OriginStatus.eq(SchemaOriginStatus::Linked.as_db_str()))
        .filter(schema::Column::OriginUpdatedAt.is_null())
        .order_by_desc(template::Column::UpdatedAt)
        .order_by_asc(schema::Column::Id)
        .limit(page.limit())
        .offset(page.offset())
        .into_model::<Row>()
        .all(&ctx.db)
        .await
        .internal()?;
    let txn = crate::db::begin_workspace_list(ctx, actor.tenant_id, &workspaces).await?;
    let mut schemas: HashMap<_, _> = schema::Entity::find()
        .filter(schema::Column::TenantId.eq(actor.tenant_id))
        .filter(schema::Column::Id.is_in(rows.iter().map(|row| row.schema_id)))
        .all(&txn)
        .await
        .internal()?
        .into_iter()
        .map(|schema| (schema.id, schema))
        .collect();
    txn.commit().await.internal()?;

    rows.into_iter()
        .filter_map(|row| schemas.remove(&row.schema_id).map(|schema| (row, schema)))
        .filter(|(row, schema)| schema.origin_template_id == Some(row.template_id))
        .map(|(row, schema)| {
            let schema_definition: MetaSchemaDefinition = serde_json::from_value(schema.definition)
                .map_err(|err| YorishiroError::Internal(err.into()))?;
            let base: MetaSchemaDefinition = schema
                .origin_snapshot
                .ok_or_else(|| YorishiroError::Internal("linked schema has no merge base".into()))
                .and_then(|value| {
                    serde_json::from_value(value)
                        .map_err(|err| YorishiroError::Internal(err.into()))
                })?;
            let template_definition: MetaSchemaDefinition =
                serde_json::from_value(row.template_definition)
                    .map_err(|err| YorishiroError::Internal(err.into()))?;
            let summary = merge::three_way(&base, &template_definition, &schema_definition).summary;
            Ok(UpstreamChange {
                workspace_id: row.workspace_id,
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
