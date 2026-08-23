//! The template marketplace: `identity_templates` (owned by base) and the two tables this crate's own migration adds to support it, `identity_template_versions` and `identity_template_reviews`.
//!
//! Record shapes, input DTOs, and the reads live here.
//! A write that is a genuine decision (ownership, status validation, version numbering under lock, a rating range) stays in `services::marketplace`, which calls into this module for the insert/update itself.

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::content_entities::DEFAULT_LIST_LIMIT;

/// A template as seen from the marketplace, with the aggregates a browser needs to choose one.
#[derive(Debug, Serialize, FromQueryResult)]
pub struct MarketplaceListing {
    pub template_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    /// The tenant that publishes it.
    /// Present so a browser can tell two same-named templates apart, not for display of anything
    /// tenant-private.
    pub tenant_id: Uuid,
    /// Highest `stable` version, or `null` when only pre-releases have been published.
    pub latest_stable_version: Option<i32>,
    pub review_count: i64,
    /// Mean rating, `null` when nobody has reviewed it.
    pub average_rating: Option<f64>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct TemplateVersionRecord {
    pub id: Uuid,
    pub template_id: Uuid,
    pub version: i32,
    pub definition: Value,
    pub changelog: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct TemplateReviewRecord {
    pub id: Uuid,
    pub template_id: Uuid,
    pub tenant_id: Uuid,
    pub rating: i16,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct PublishVersionRequest {
    pub definition: Value,
    pub changelog: Option<String>,
    /// `draft` (default), `pre`, or `stable`.
    #[serde(default = "default_status")]
    pub status: String,
}

fn default_status() -> String {
    "draft".to_string()
}

#[derive(Debug, Deserialize)]
pub struct SubmitReviewRequest {
    pub rating: i16,
    pub comment: Option<String>,
}

/// The template columns a fork copies across, minus the definition (which comes from the chosen
/// version rather than the template row).
#[derive(FromQueryResult)]
pub(crate) struct ForkSource {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) author: Option<String>,
}

/// `GET /hosted/marketplace`'s `limit`/`offset`, clamped the same way `entities::list`'s are.
#[derive(Debug, Clone, Copy)]
pub struct ListMarketplaceQuery {
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListMarketplaceQuery {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIST_LIMIT,
            offset: 0,
        }
    }
}

/// Lists community-visible templates across every tenant, ordered by name then id, one page at a time.
///
/// A template appears only once it has a non-draft version: `visibility = 'community'` says its owner is willing to share it, but with nothing published there is nothing to install, and a listing whose every entry 404s on install is worse than a shorter one.
///
/// The three aggregates (latest stable version, review count, average rating) stay correlated subqueries rather than a `JOIN` + `GROUP BY`: with `limit` bounding the page to at most 200 rows, their cost is bounded by the page size, and nothing here has measured the join rewrite as faster.
pub async fn list_marketplace(
    conn: &impl ConnectionTrait,
    query: ListMarketplaceQuery,
) -> Result<Vec<MarketplaceListing>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);

    MarketplaceListing::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT t.id AS template_id, t.name, t.description, t.tags, t.author, t.tenant_id, \
         (SELECT max(v.version) FROM identity_template_versions v \
           WHERE v.template_id = t.id AND v.status = 'stable') AS latest_stable_version, \
         (SELECT count(*) FROM identity_template_reviews r \
           WHERE r.template_id = t.id) AS review_count, \
         (SELECT avg(r.rating)::float8 FROM identity_template_reviews r \
           WHERE r.template_id = t.id) AS average_rating \
         FROM identity_templates t \
         WHERE t.visibility = 'community' \
           AND EXISTS ( \
             SELECT 1 FROM identity_template_versions v \
              WHERE v.template_id = t.id AND v.status <> 'draft' \
           ) \
         ORDER BY t.name ASC, t.id ASC \
         LIMIT $1 OFFSET $2",
        [limit.into(), offset.into()],
    ))
    .all(conn)
    .await
    .internal()
}

/// Versions of a template that `tenant_id` is allowed to see.
///
/// **Drafts are the caller's own only.** The database does not enforce this:
/// `identity_template_versions` carries no RLS, matching `identity_templates`, so this WHERE
/// clause is the enforcement, and dropping it publishes every tenant's unfinished work.
pub async fn list_versions(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<Vec<TemplateVersionRecord>, YorishiroError> {
    TemplateVersionRecord::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT v.id, v.template_id, v.version, v.definition, v.changelog, v.status, \
                v.created_at \
           FROM identity_template_versions v \
           JOIN identity_templates t ON t.id = v.template_id \
          WHERE v.template_id = $1 \
            AND (t.tenant_id = $2 OR t.visibility = 'community') \
            AND (v.status <> 'draft' OR t.tenant_id = $2) \
          ORDER BY v.version DESC",
        [template_id.into(), tenant_id.into()],
    ))
    .all(conn)
    .await
    .internal()
}

/// Inserts the next version of a template, the number assigned as `max(version) + 1` inside the same statement.
///
/// Called inside the transaction that holds `template-version:{template_id}`'s advisory lock (`services::marketplace::publish_version`): at READ COMMITTED, Postgres locks no range for rows that do not exist yet, so two concurrent inserts would otherwise read the same maximum and collide on the `template_id, version` unique index.
pub(crate) async fn insert_next_version(
    conn: &impl ConnectionTrait,
    template_id: Uuid,
    request: &PublishVersionRequest,
    user_id: Option<Uuid>,
) -> Result<TemplateVersionRecord, YorishiroError> {
    TemplateVersionRecord::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO identity_template_versions \
                (template_id, version, definition, changelog, status, created_by) \
         SELECT $1, \
                COALESCE(max(v.version), 0) + 1, \
                $2, $3, $4, $5 \
           FROM identity_template_versions v \
          WHERE v.template_id = $1 \
         RETURNING id, template_id, version, definition, changelog, status, created_at",
        [
            template_id.into(),
            request.definition.clone().into(),
            request.changelog.clone().into(),
            request.status.clone().into(),
            user_id.into(),
        ],
    ))
    .one(conn)
    .await
    .internal()?
    .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("insert did not return a row")))
}

/// Reviews of a template, readable by anyone who can see the template itself.
pub async fn list_reviews(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<Vec<TemplateReviewRecord>, YorishiroError> {
    TemplateReviewRecord::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT r.id, r.template_id, r.tenant_id, r.rating, r.comment, r.created_at, \
                r.updated_at \
           FROM identity_template_reviews r \
           JOIN identity_templates t ON t.id = r.template_id \
          WHERE r.template_id = $1 \
            AND (t.tenant_id = $2 OR t.visibility = 'community') \
          ORDER BY r.created_at DESC",
        [template_id.into(), tenant_id.into()],
    ))
    .all(conn)
    .await
    .internal()
}

/// Whether a template visible to `tenant_id` exists, for
/// `services::marketplace::submit_review`'s "reviewing a template nobody can see is meaningless"
/// guard.
pub(crate) async fn is_visible(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<bool, YorishiroError> {
    #[derive(FromQueryResult)]
    struct Exists {
        exists: bool,
    }
    let row = Exists::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT EXISTS(SELECT 1 FROM identity_templates \
          WHERE id = $1 AND (tenant_id = $2 OR visibility = 'community')) AS exists",
        [template_id.into(), tenant_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;
    Ok(row.is_some_and(|r| r.exists))
}

/// Records this tenant's review, replacing its previous one if it had left one.
pub(crate) async fn upsert_review(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
    user_id: Option<Uuid>,
    request: &SubmitReviewRequest,
) -> Result<TemplateReviewRecord, YorishiroError> {
    TemplateReviewRecord::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO identity_template_reviews \
                (template_id, tenant_id, rating, comment, created_by) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (template_id, tenant_id) DO UPDATE \
             SET rating = EXCLUDED.rating, \
                 comment = EXCLUDED.comment, \
                 updated_at = now() \
         RETURNING id, template_id, tenant_id, rating, comment, created_at, updated_at",
        [
            template_id.into(),
            tenant_id.into(),
            request.rating.into(),
            request.comment.clone().into(),
            user_id.into(),
        ],
    ))
    .one(conn)
    .await
    .internal()?
    .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("upsert did not return a row")))
}

/// The source template's copyable columns, if `tenant_id` may see it.
pub(crate) async fn find_fork_source(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<Option<ForkSource>, YorishiroError> {
    ForkSource::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT name, description, tags, author FROM identity_templates \
          WHERE id = $1 AND (tenant_id = $2 OR visibility = 'community')",
        [template_id.into(), tenant_id.into()],
    ))
    .one(conn)
    .await
    .internal()
}

/// The definition to fork: the given version if published (never a draft, even by number), or
/// the latest `stable` one when `version` is `None`.
pub(crate) async fn find_forkable_definition(
    conn: &impl ConnectionTrait,
    template_id: Uuid,
    version: Option<i32>,
) -> Result<Option<Value>, YorishiroError> {
    #[derive(FromQueryResult)]
    struct Definition {
        definition: Value,
    }

    let row = match version {
        Some(version) => {
            Definition::find_by_statement(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT definition FROM identity_template_versions \
                  WHERE template_id = $1 AND version = $2 AND status <> 'draft'",
                [template_id.into(), version.into()],
            ))
            .one(conn)
            .await
        }
        None => {
            Definition::find_by_statement(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT definition FROM identity_template_versions \
                  WHERE template_id = $1 AND status = 'stable' \
                  ORDER BY version DESC LIMIT 1",
                [template_id.into()],
            ))
            .one(conn)
            .await
        }
    }
    .internal()?;

    Ok(row.map(|r| r.definition))
}

/// Outcome of [`insert_fork`]: either the new template's id, or the name it collided on.
pub(crate) enum InsertForkOutcome {
    Created(Uuid),
    NameTaken,
}

/// Inserts the caller's own private copy of a forked template.
/// The forked copy starts `visibility = 'tenant'`: publishing someone else's work into the
/// marketplace under your name is a decision, not a default.
pub(crate) async fn insert_fork(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    source: &ForkSource,
    definition: &Value,
    fork_of: Uuid,
    user_id: Option<Uuid>,
) -> Result<InsertForkOutcome, YorishiroError> {
    #[derive(FromQueryResult)]
    struct Inserted {
        id: Uuid,
    }

    let result = Inserted::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO identity_templates \
                (tenant_id, name, description, definition, tags, author, visibility, fork_of, \
                 created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, 'tenant', $7, $8) \
         RETURNING id",
        [
            tenant_id.into(),
            source.name.clone().into(),
            source.description.clone().into(),
            definition.clone().into(),
            source.tags.clone().into(),
            source.author.clone().into(),
            fork_of.into(),
            user_id.into(),
        ],
    ))
    .one(conn)
    .await;

    match result {
        Err(sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(err)))
            if err
                .as_database_error()
                .is_some_and(|db_err| db_err.is_unique_violation()) =>
        {
            Ok(InsertForkOutcome::NameTaken)
        }
        other => {
            let row = other.internal()?.ok_or_else(|| {
                YorishiroError::Internal(anyhow::anyhow!("insert returned no row"))
            })?;
            Ok(InsertForkOutcome::Created(row.id))
        }
    }
}

/// Whether a template exists and belongs to `tenant_id`, for
/// `services::marketplace::require_ownership`.
pub(crate) async fn is_owned_by(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<bool, YorishiroError> {
    #[derive(FromQueryResult)]
    struct Exists {
        exists: bool,
    }
    let row = Exists::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT EXISTS(SELECT 1 FROM identity_templates WHERE id = $1 AND tenant_id = $2) \
         AS exists",
        [template_id.into(), tenant_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;
    Ok(row.is_some_and(|r| r.exists))
}

/// Sets a template's marketplace visibility.
/// `services::marketplace::set_visibility` has already checked ownership; this is the write
/// alone.
pub(crate) async fn update_visibility(
    conn: &impl ConnectionTrait,
    template_id: Uuid,
    visibility: &str,
) -> Result<(), YorishiroError> {
    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE identity_templates SET visibility = $1, updated_at = now() WHERE id = $2",
        [visibility.into(), template_id.into()],
    ))
    .await
    .internal()?;
    Ok(())
}
