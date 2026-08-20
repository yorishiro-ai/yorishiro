//! Reading which schemas have an origin template that moved on without them.
//!
//! The query alone: what to do about a change is `services::origin`'s, which decides whether a merge is safe and what it would produce.
//!
//! Owns no table. `content.schemas` and `identity.templates` are both base's, read here over the control-plane pool because the request role cannot see `identity.templates` at all.
//! That does not make this base's: the endpoint it serves is enterprise, and an edition is decided by what a feature is rather than by which tables it reads.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::models::schemas::UpstreamChange;
use yorishiro_core::{ResultExt, YorishiroError};

/// Schemas in this workspace whose origin template has changed since the copy was taken.
///
/// Nothing is applied.
/// The upstream edit does not reach the copy on its own (an automatic update could make stored entities invalid against a definition nobody here chose), so this reports and the workspace decides.
///
/// A schema whose template was deleted is not reported: the trigger has already detached it, and there is no longer an update to take.
/// `linked` is the whole population here.
pub async fn list_with_upstream_changes(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<Vec<UpstreamChange>, YorishiroError> {
    // Joins identity.templates, which the request role cannot read (the base spec §2.3), so this runs on the control-plane pool like the rest of the template-library paths.
    let rows: Vec<(Uuid, String, i32, Uuid, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT s.id, s.name, s.version, t.id, t.name, t.updated_at          FROM content.schemas s          JOIN identity.templates t ON t.id = s.origin_template_id          WHERE s.workspace_id = $1            AND s.status = 'active'            AND s.origin_status = 'linked'            AND t.updated_at > s.created_at          ORDER BY t.updated_at DESC",
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await
    .internal()?;

    Ok(rows
        .into_iter()
        .map(
            |(schema_id, schema_name, version, template_id, template_name, changed_at)| {
                UpstreamChange {
                    schema_id,
                    schema_name,
                    version,
                    template_id,
                    template_name,
                    changed_at,
                }
            },
        )
        .collect())
}
