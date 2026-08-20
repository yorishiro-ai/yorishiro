//! The template marketplace: `identity.templates` and the two tables this repo's own migration adds to support it, `template_versions` and `template_reviews`.
//!
//! Record shapes, input DTOs, and the reads live here.
//! A write that is a genuine decision (ownership, status validation, version numbering under lock, a rating range) stays in `services::marketplace`, which calls into this module for the insert/update itself.

use chrono::{DateTime, Utc};
use sea_query::{
    Alias, Asterisk, Expr, Func, Iden, Order, PostgresQueryBuilder, Query, SimpleExpr,
};
use sea_query_binder::SqlxBinder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgConnection, PgPool, Row};
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::ResultExt;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::models::entities::DEFAULT_LIST_LIMIT;

#[derive(Iden)]
enum Templates {
    Table,
    Id,
    Name,
    Description,
    Tags,
    Author,
    TenantId,
    Visibility,
}

#[derive(Iden)]
enum TemplateVersions {
    Table,
    TemplateId,
    Version,
    Status,
}

#[derive(Iden)]
enum TemplateReviews {
    Table,
    TemplateId,
    Rating,
}

/// A template as seen from the marketplace, with the aggregates a browser needs to choose one.
#[derive(Debug, Serialize, ToSchema)]
pub struct MarketplaceListing {
    pub template_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    /// The tenant that publishes it.
    /// Present so a browser can tell two same-named templates apart, not for display of anything tenant-private.
    pub tenant_id: Uuid,
    /// Highest `stable` version, or `null` when only pre-releases have been published.
    pub latest_stable_version: Option<i32>,
    pub review_count: i64,
    /// Mean rating, `null` when nobody has reviewed it.
    pub average_rating: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TemplateVersionRecord {
    pub id: Uuid,
    pub template_id: Uuid,
    pub version: i32,
    pub definition: Value,
    pub changelog: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TemplateReviewRecord {
    pub id: Uuid,
    pub template_id: Uuid,
    pub tenant_id: Uuid,
    pub rating: i16,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct SubmitReviewRequest {
    pub rating: i16,
    pub comment: Option<String>,
}

/// The template columns a fork copies across, minus the definition (which comes from the chosen version rather than the template row).
#[derive(sqlx::FromRow)]
pub(crate) struct ForkSource {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) author: Option<String>,
}

/// `GET /api/marketplace`'s `limit`/`offset`, clamped the same way `entities::list`'s are.
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
/// The three aggregates (latest stable version, review count, average rating) stay correlated subqueries rather than a `JOIN` + `GROUP BY`: with `limit` now bounding the page to at most 200 rows, their cost is bounded by the page size, and nothing here has measured the join rewrite as faster.
pub async fn list_marketplace(
    pool: &PgPool,
    query: ListMarketplaceQuery,
) -> Result<Vec<MarketplaceListing>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);

    let latest_stable_version = SimpleExpr::SubQuery(
        None,
        Box::new(
            Query::select()
                .expr(Func::max(Expr::col(TemplateVersions::Version)))
                .from((Alias::new("identity"), TemplateVersions::Table))
                .and_where(
                    Expr::col(TemplateVersions::TemplateId)
                        .equals((Templates::Table, Templates::Id)),
                )
                .and_where(Expr::col(TemplateVersions::Status).eq("stable"))
                .to_owned()
                .into_sub_query_statement(),
        ),
    );
    let review_count = SimpleExpr::SubQuery(
        None,
        Box::new(
            Query::select()
                .expr(Func::count(Expr::col(Asterisk)))
                .from((Alias::new("identity"), TemplateReviews::Table))
                .and_where(
                    Expr::col(TemplateReviews::TemplateId)
                        .equals((Templates::Table, Templates::Id)),
                )
                .to_owned()
                .into_sub_query_statement(),
        ),
    );
    let average_rating = SimpleExpr::SubQuery(
        None,
        Box::new(
            Query::select()
                .expr(Func::cast_as(
                    Func::avg(Expr::col(TemplateReviews::Rating)),
                    Alias::new("float8"),
                ))
                .from((Alias::new("identity"), TemplateReviews::Table))
                .and_where(
                    Expr::col(TemplateReviews::TemplateId)
                        .equals((Templates::Table, Templates::Id)),
                )
                .to_owned()
                .into_sub_query_statement(),
        ),
    );
    let has_published_version = Expr::exists(
        Query::select()
            .expr(Expr::val(1))
            .from((Alias::new("identity"), TemplateVersions::Table))
            .and_where(
                Expr::col(TemplateVersions::TemplateId).equals((Templates::Table, Templates::Id)),
            )
            .and_where(Expr::col(TemplateVersions::Status).ne("draft"))
            .to_owned(),
    );

    let (sql, values) = Query::select()
        .column((Templates::Table, Templates::Id))
        .column((Templates::Table, Templates::Name))
        .column((Templates::Table, Templates::Description))
        .column((Templates::Table, Templates::Tags))
        .column((Templates::Table, Templates::Author))
        .column((Templates::Table, Templates::TenantId))
        .expr_as(latest_stable_version, Alias::new("latest_stable_version"))
        .expr_as(review_count, Alias::new("review_count"))
        .expr_as(average_rating, Alias::new("average_rating"))
        .from((Alias::new("identity"), Templates::Table))
        .and_where(Expr::col(Templates::Visibility).eq("community"))
        .and_where(has_published_version)
        // `name` alone is not a stable order: it is unique per tenant, not globally, so two
        // tenants can publish the same name and tie. `id` breaks that tie deterministically,
        // which a page split across two requests needs.
        .order_by((Templates::Table, Templates::Name), Order::Asc)
        .order_by((Templates::Table, Templates::Id), Order::Asc)
        .limit(limit as u64)
        .offset(offset as u64)
        .build_sqlx(PostgresQueryBuilder);

    let rows = sqlx::query_with(&sql, values)
        .fetch_all(pool)
        .await
        .internal()?;

    rows.into_iter()
        .map(|row| {
            Ok(MarketplaceListing {
                template_id: row.try_get("id").internal()?,
                name: row.try_get("name").internal()?,
                description: row.try_get("description").internal()?,
                tags: row.try_get("tags").internal()?,
                author: row.try_get("author").internal()?,
                tenant_id: row.try_get("tenant_id").internal()?,
                latest_stable_version: row.try_get("latest_stable_version").internal()?,
                review_count: row.try_get("review_count").internal()?,
                average_rating: row.try_get("average_rating").internal()?,
            })
        })
        .collect()
}

/// Versions of a template that `tenant_id` is allowed to see.
///
/// **Drafts are the caller's own only.** The database does not enforce this: `template_versions` carries no RLS, matching `identity.templates`, so this WHERE clause is the enforcement, and dropping it publishes every tenant's unfinished work.
pub async fn list_versions(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<Vec<TemplateVersionRecord>, YorishiroError> {
    let rows = sqlx::query(
        r#"
        SELECT v.id, v.template_id, v.version, v.definition, v.changelog, v.status, v.created_at
          FROM identity.template_versions v
          JOIN identity.templates t ON t.id = v.template_id
         WHERE v.template_id = $1
           AND (t.tenant_id = $2 OR t.visibility = 'community')
           AND (v.status <> 'draft' OR t.tenant_id = $2)
         ORDER BY v.version DESC
        "#,
    )
    .bind(template_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .internal()?;

    rows.into_iter()
        .map(|row| {
            Ok(TemplateVersionRecord {
                id: row.try_get("id").internal()?,
                template_id: row.try_get("template_id").internal()?,
                version: row.try_get("version").internal()?,
                definition: row.try_get("definition").internal()?,
                changelog: row.try_get("changelog").internal()?,
                status: row.try_get("status").internal()?,
                created_at: row.try_get("created_at").internal()?,
            })
        })
        .collect()
}

/// Inserts the next version of a template, the number assigned as `max(version) + 1` inside the same statement.
///
/// Takes `&mut PgConnection` rather than `&PgPool` so `services::marketplace::publish_version` can run this inside the transaction that holds `template-version:{template_id}`'s advisory lock: at READ COMMITTED, Postgres locks no range for rows that do not exist yet, so two concurrent inserts would otherwise read the same maximum and collide on `UNIQUE (template_id, version)`.
pub(crate) async fn insert_next_version(
    conn: &mut PgConnection,
    template_id: Uuid,
    request: &PublishVersionRequest,
    user_id: Option<Uuid>,
) -> Result<TemplateVersionRecord, YorishiroError> {
    let row = sqlx::query(
        r#"
        INSERT INTO identity.template_versions
               (template_id, version, definition, changelog, status, created_by)
        SELECT $1,
               COALESCE(max(v.version), 0) + 1,
               $2, $3, $4, $5
          FROM identity.template_versions v
         WHERE v.template_id = $1
        RETURNING id, template_id, version, definition, changelog, status, created_at
        "#,
    )
    .bind(template_id)
    .bind(&request.definition)
    .bind(&request.changelog)
    .bind(&request.status)
    .bind(user_id)
    .fetch_one(&mut *conn)
    .await
    .internal()?;

    Ok(TemplateVersionRecord {
        id: row.try_get("id").internal()?,
        template_id: row.try_get("template_id").internal()?,
        version: row.try_get("version").internal()?,
        definition: row.try_get("definition").internal()?,
        changelog: row.try_get("changelog").internal()?,
        status: row.try_get("status").internal()?,
        created_at: row.try_get("created_at").internal()?,
    })
}

/// Reviews of a template, readable by anyone who can see the template itself.
pub async fn list_reviews(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<Vec<TemplateReviewRecord>, YorishiroError> {
    let rows = sqlx::query(
        r#"
        SELECT r.id, r.template_id, r.tenant_id, r.rating, r.comment, r.created_at, r.updated_at
          FROM identity.template_reviews r
          JOIN identity.templates t ON t.id = r.template_id
         WHERE r.template_id = $1
           AND (t.tenant_id = $2 OR t.visibility = 'community')
         ORDER BY r.created_at DESC
        "#,
    )
    .bind(template_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .internal()?;

    rows.into_iter()
        .map(|row| {
            Ok(TemplateReviewRecord {
                id: row.try_get("id").internal()?,
                template_id: row.try_get("template_id").internal()?,
                tenant_id: row.try_get("tenant_id").internal()?,
                rating: row.try_get("rating").internal()?,
                comment: row.try_get("comment").internal()?,
                created_at: row.try_get("created_at").internal()?,
                updated_at: row.try_get("updated_at").internal()?,
            })
        })
        .collect()
}

/// Whether a template visible to `tenant_id` exists, for `services::marketplace::submit_review`'s "reviewing a template nobody can see is meaningless" guard.
pub(crate) async fn is_visible(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<bool, YorishiroError> {
    let visible: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM identity.templates \
         WHERE id = $1 AND (tenant_id = $2 OR visibility = 'community')",
    )
    .bind(template_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .internal()?;
    Ok(visible.is_some())
}

/// Records this tenant's review, replacing its previous one if it had left one.
pub(crate) async fn upsert_review(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
    user_id: Option<Uuid>,
    request: &SubmitReviewRequest,
) -> Result<TemplateReviewRecord, YorishiroError> {
    let row = sqlx::query(
        r#"
        INSERT INTO identity.template_reviews
               (template_id, tenant_id, rating, comment, created_by)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (template_id, tenant_id) DO UPDATE
            SET rating = EXCLUDED.rating,
                comment = EXCLUDED.comment,
                updated_at = now()
        RETURNING id, template_id, tenant_id, rating, comment, created_at, updated_at
        "#,
    )
    .bind(template_id)
    .bind(tenant_id)
    .bind(request.rating)
    .bind(&request.comment)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .internal()?;

    Ok(TemplateReviewRecord {
        id: row.try_get("id").internal()?,
        template_id: row.try_get("template_id").internal()?,
        tenant_id: row.try_get("tenant_id").internal()?,
        rating: row.try_get("rating").internal()?,
        comment: row.try_get("comment").internal()?,
        created_at: row.try_get("created_at").internal()?,
        updated_at: row.try_get("updated_at").internal()?,
    })
}

/// The source template's copyable columns, if `tenant_id` may see it.
pub(crate) async fn find_fork_source(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<Option<ForkSource>, YorishiroError> {
    sqlx::query_as(
        "SELECT name, description, tags, author FROM identity.templates \
         WHERE id = $1 AND (tenant_id = $2 OR visibility = 'community')",
    )
    .bind(template_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .internal()
}

/// The definition to fork: the given version if published (never a draft, even by number), or the latest `stable` one when `version` is `None`.
pub(crate) async fn find_forkable_definition(
    pool: &PgPool,
    template_id: Uuid,
    version: Option<i32>,
) -> Result<Option<Value>, YorishiroError> {
    let definition: Option<(Value,)> = match version {
        Some(version) => sqlx::query_as(
            "SELECT definition FROM identity.template_versions \
             WHERE template_id = $1 AND version = $2 AND status <> 'draft'",
        )
        .bind(template_id)
        .bind(version)
        .fetch_optional(pool)
        .await
        .internal()?,
        None => sqlx::query_as(
            "SELECT definition FROM identity.template_versions \
             WHERE template_id = $1 AND status = 'stable' \
             ORDER BY version DESC LIMIT 1",
        )
        .bind(template_id)
        .fetch_optional(pool)
        .await
        .internal()?,
    };
    Ok(definition.map(|(d,)| d))
}

/// Outcome of [`insert_fork`]: either the new template's id, or the name it collided on.
pub(crate) enum InsertForkOutcome {
    Created(Uuid),
    NameTaken,
}

/// Inserts the caller's own private copy of a forked template.
/// The forked copy starts `visibility = 'tenant'`: publishing someone else's work into the marketplace under your name is a decision, not a default.
pub(crate) async fn insert_fork(
    pool: &PgPool,
    tenant_id: Uuid,
    source: &ForkSource,
    definition: &Value,
    fork_of: Uuid,
    user_id: Option<Uuid>,
) -> Result<InsertForkOutcome, YorishiroError> {
    let inserted = sqlx::query(
        r#"
        INSERT INTO identity.templates
               (tenant_id, name, description, definition, tags, author, visibility, fork_of, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, 'tenant', $7, $8)
        RETURNING id
        "#,
    )
    .bind(tenant_id)
    .bind(&source.name)
    .bind(&source.description)
    .bind(definition)
    .bind(&source.tags)
    .bind(&source.author)
    .bind(fork_of)
    .bind(user_id)
    .fetch_one(pool)
    .await;

    match inserted {
        Err(err)
            if err
                .as_database_error()
                .is_some_and(|db_err| db_err.is_unique_violation()) =>
        {
            Ok(InsertForkOutcome::NameTaken)
        }
        other => {
            let row = other.internal()?;
            Ok(InsertForkOutcome::Created(row.try_get("id").internal()?))
        }
    }
}

/// Whether a template exists and belongs to `tenant_id`, for `services::marketplace::require_ownership`.
pub(crate) async fn is_owned_by(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<bool, YorishiroError> {
    let owned: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM identity.templates WHERE id = $1 AND tenant_id = $2")
            .bind(template_id)
            .bind(tenant_id)
            .fetch_optional(pool)
            .await
            .internal()?;
    Ok(owned.is_some())
}

/// Sets a template's marketplace visibility.
/// `services::marketplace::set_visibility` has already checked ownership; this is the write alone.
pub(crate) async fn update_visibility(
    pool: &PgPool,
    template_id: Uuid,
    visibility: &str,
) -> Result<(), YorishiroError> {
    sqlx::query("UPDATE identity.templates SET visibility = $1, updated_at = now() WHERE id = $2")
        .bind(visibility)
        .bind(template_id)
        .execute(pool)
        .await
        .internal()?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/models/marketplace.rs"]
mod tests;
