//! Reading which schemas have an origin template that moved on without them.
//!
//! Template metadata is joined on the control plane to page the candidates.
//! Workspace schema definitions are loaded only on the RLS-scoped connection.

use super::merge;
use crate::error::OptionExt;
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
/// Nothing is applied: the workspace decides whether to follow the upstream edit.
///
/// A schema whose template was deleted is not reported, because deleting the template detaches it.
///
/// `origin_updated_at IS NULL` marks a pending change.
/// It survives local schema versioning and is cleared only by a merge.
pub(crate) async fn list_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::access_keys::AuthContext,
    page: ListParams,
) -> Result<Vec<UpstreamChange>, YorishiroError> {
    use crate::models::_entities::{schema_schemas as schema, template_templates as template};
    use crate::models::iam::{access::read_workspaces, policy::Action};
    use std::collections::HashMap;
    let workspaces = read_workspaces(&ctx.db, actor, Action::ListSchemaUpstreamChanges).await?;
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
            let schema_definition: MetaSchemaDefinition =
                serde_json::from_value(schema.definition).internal()?;
            let base: MetaSchemaDefinition = schema
                .origin_snapshot
                .or_internal("linked schema has no merge base")
                .and_then(|value| serde_json::from_value(value).internal())?;
            let template_definition: MetaSchemaDefinition =
                serde_json::from_value(row.template_definition).internal()?;
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
