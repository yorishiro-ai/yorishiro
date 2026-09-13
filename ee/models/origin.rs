//! Reading which schemas have an origin template that moved on without them.
//!
//! The query alone: what to do about a change is `services::origin`'s, which decides whether a merge is safe and what it would produce.
//!
//! Owns no table.
//! `schema_schemas` and `template_templates` are both base's; both are read here on `ctx.db` (the migration/admin connection), since `template_templates` carries no GRANT to `yorishiro_app` and a request's RLS-scoped connection cannot see it at all.
//! That does not make this base's: the endpoint it serves is enterprise, and an edition is decided by what a feature is rather than by which tables it reads.

use crate::ee::services::merge;
use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::MetaSchemaDefinition;
use crate::models::pagination::ListParams;
use crate::models::schema_schemas::UpstreamChange;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
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
/// Stays raw SQL because this projection joins control-plane and workspace tables and computes its merge summary from both definitions.
pub async fn list_with_upstream_changes(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    page: ListParams,
) -> Result<Vec<UpstreamChange>, YorishiroError> {
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT s.id AS schema_id, s.name AS schema_name, s.version, \
                t.id AS template_id, t.name AS template_name, t.updated_at AS changed_at, s.origin_updated_at AS origin_updated_at, \
                s.definition AS schema_definition, s.origin_snapshot AS origin_snapshot, t.definition AS template_definition \
           FROM schema_schemas s \
           JOIN template_templates t ON t.id = s.origin_template_id \
          WHERE s.workspace_id = $1 \
            AND s.status = 'active' \
            AND s.origin_status = 'linked' \
            AND s.origin_updated_at IS NULL \
          ORDER BY t.updated_at DESC \
          LIMIT $2 OFFSET $3",
        [
            workspace_id.into(),
            page.limit().into(),
            page.offset().into(),
        ],
    ))
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
