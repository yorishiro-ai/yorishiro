//! Reading which schemas have an origin template that moved on without them.
//!
//! The query alone: what to do about a change is `services::origin`'s, which decides whether a merge is safe and what it would produce.
//!
//! Owns no table.
//! `content_schemas` and `identity_templates` are both base's; both are read here on `ctx.db` (the migration/admin connection), since `identity_templates` carries no GRANT to `yorishiro_app` and a request's RLS-scoped connection cannot see it at all.
//! That does not make this base's: the endpoint it serves is enterprise, and an edition is decided by what a feature is rather than by which tables it reads.

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::content_schemas::UpstreamChange;
use yorishiro_core::models::pagination::ListParams;

#[derive(FromQueryResult)]
struct Row {
    schema_id: Uuid,
    schema_name: String,
    version: i32,
    template_id: Uuid,
    template_name: String,
    changed_at: DateTime<Utc>,
}

/// Schemas in this workspace whose origin template has changed since the copy was taken.
///
/// Nothing is applied.
/// The upstream edit does not reach the copy on its own (an automatic update could make stored entities invalid against a definition nobody here chose), so this reports and the workspace decides.
///
/// A schema whose template was deleted is not reported: the trigger has already detached it, and there is no longer an update to take.
/// `linked` is the whole population here.
pub async fn list_with_upstream_changes(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    page: ListParams,
) -> Result<Vec<UpstreamChange>, YorishiroError> {
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT s.id AS schema_id, s.name AS schema_name, s.version, \
                t.id AS template_id, t.name AS template_name, t.updated_at AS changed_at \
           FROM content_schemas s \
           JOIN identity_templates t ON t.id = s.origin_template_id \
          WHERE s.workspace_id = $1 \
            AND s.status = 'active' \
            AND s.origin_status = 'linked' \
            AND t.updated_at > s.created_at \
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

    Ok(rows
        .into_iter()
        .map(|row| UpstreamChange {
            schema_id: row.schema_id,
            schema_name: row.schema_name,
            version: row.version,
            template_id: row.template_id,
            template_name: row.template_name,
            changed_at: row.changed_at,
        })
        .collect())
}
