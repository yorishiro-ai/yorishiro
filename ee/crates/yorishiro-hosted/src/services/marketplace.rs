use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::ResultExt;
use yorishiro_core::db;
use yorishiro_core::error::YorishiroError;

/// A template as seen from the marketplace, with the aggregates a browser needs to choose one.
#[derive(Debug, Serialize, ToSchema)]
pub struct MarketplaceListing {
    pub template_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    /// The tenant that publishes it. Present so a browser can tell two same-named templates
    /// apart, not for display of anything tenant-private.
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

/// The template columns a fork copies across, minus the definition (which comes from the chosen
/// version rather than the template row).
#[derive(sqlx::FromRow)]
struct ForkSource {
    name: String,
    description: Option<String>,
    tags: Vec<String>,
    author: Option<String>,
}

fn validate_status(status: &str) -> Result<(), YorishiroError> {
    if matches!(status, "draft" | "pre" | "stable") {
        Ok(())
    } else {
        Err(YorishiroError::ValidationFailed {
            message: format!("unknown publish status '{status}'"),
            details: Vec::new(),
            hint: "use one of: draft, pre, stable".into(),
        })
    }
}

/// Lists community-visible templates across every tenant.
///
/// A template appears only once it has a non-draft version: `visibility = 'community'` says its
/// owner is willing to share it, but with nothing published there is nothing to install, and a
/// listing whose every entry 404s on install is worse than a shorter one.
pub async fn list_marketplace(pool: &PgPool) -> Result<Vec<MarketplaceListing>, YorishiroError> {
    let rows = sqlx::query(
        r#"
        SELECT t.id            AS template_id,
               t.name          AS name,
               t.description   AS description,
               t.tags          AS tags,
               t.author        AS author,
               t.tenant_id     AS tenant_id,
               (SELECT max(v.version) FROM identity.template_versions v
                 WHERE v.template_id = t.id AND v.status = 'stable') AS latest_stable_version,
               (SELECT count(*) FROM identity.template_reviews r
                 WHERE r.template_id = t.id)                          AS review_count,
               (SELECT avg(r.rating)::float8 FROM identity.template_reviews r
                 WHERE r.template_id = t.id)                          AS average_rating
          FROM identity.templates t
         WHERE t.visibility = 'community'
           AND EXISTS (
                 SELECT 1 FROM identity.template_versions v
                  WHERE v.template_id = t.id AND v.status <> 'draft'
               )
         ORDER BY t.name
        "#,
    )
    .fetch_all(pool)
    .await
    .internal()?;

    rows.into_iter()
        .map(|row| {
            Ok(MarketplaceListing {
                template_id: row.try_get("template_id").internal()?,
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
/// **Drafts are the caller's own only.** The database does not enforce this -- `template_versions`
/// carries no RLS, matching `identity.templates` -- so this WHERE clause is the enforcement, and
/// dropping it publishes every tenant's unfinished work.
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

/// Publishes the next version of a template.
///
/// Only the owning tenant may publish, and the version number is assigned here rather than taken
/// from the caller -- letting a client choose it invites gaps and collisions in a sequence other
/// tenants read as history.
pub async fn publish_version(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
    user_id: Option<Uuid>,
    request: PublishVersionRequest,
) -> Result<TemplateVersionRecord, YorishiroError> {
    validate_status(&request.status)?;
    require_ownership(pool, tenant_id, template_id).await?;

    // The number comes from `max(version) + 1` read in the same statement that inserts, and at
    // READ COMMITTED Postgres locks no range for the rows that do not exist yet -- so two
    // concurrent publishes of one template both read the same maximum and both try to write the
    // same next version. `UNIQUE (template_id, version)` catches it, which is why this was never
    // corruption, but the loser got an opaque 500 for doing nothing wrong.
    //
    // Serializing on the template turns that into what the caller expects: both succeed, with
    // consecutive numbers. The lock is transaction-scoped, so it releases on commit or rollback
    // with no unlock to forget.
    let mut tx = pool.begin().await.internal()?;
    db::lock_for_update(&mut tx, &format!("template-version:{template_id}"))
        .await
        .internal()?;

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
    .fetch_one(&mut *tx)
    .await
    .internal()?;

    tx.commit().await.internal()?;

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

/// Records this tenant's review, replacing its previous one if it had left one.
///
/// `tenant_id` comes from the authenticated context, never from the request body: taking it from
/// input would let any caller review as any tenant, which is the whole value of a rating.
pub async fn submit_review(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
    user_id: Option<Uuid>,
    request: SubmitReviewRequest,
) -> Result<TemplateReviewRecord, YorishiroError> {
    if !(1..=5).contains(&request.rating) {
        return Err(YorishiroError::ValidationFailed {
            message: "rating must be between 1 and 5".into(),
            details: Vec::new(),
            hint: "send an integer rating from 1 (worst) to 5 (best)".into(),
        });
    }

    // Reviewing a template nobody can see is meaningless, and would leak that it exists.
    let visible: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM identity.templates \
         WHERE id = $1 AND (tenant_id = $2 OR visibility = 'community')",
    )
    .bind(template_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .internal()?;
    if visible.is_none() {
        return Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )));
    }

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

/// Copies a published version of someone else's template into the caller's own library.
///
/// The copy records `fork_of`, and takes the definition from the *version* rather than the
/// template row: the template keeps moving as its owner edits it, so forking "the template"
/// would install whatever it happened to be at that instant rather than the version chosen.
pub async fn fork_template(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
    version: Option<i32>,
    user_id: Option<Uuid>,
) -> Result<Uuid, YorishiroError> {
    let source: Option<ForkSource> = sqlx::query_as(
        "SELECT name, description, tags, author FROM identity.templates \
         WHERE id = $1 AND (tenant_id = $2 OR visibility = 'community')",
    )
    .bind(template_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await
    .internal()?;

    let Some(ForkSource {
        name,
        description,
        tags,
        author,
    }) = source
    else {
        return Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )));
    };

    // A draft is never forkable, even by version number: it is explicitly not published yet.
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

    let Some((definition,)) = definition else {
        return Err(YorishiroError::not_found(
            "no published version of this template is available to fork",
        ));
    };

    // The forked copy is the caller's own template, so it starts private: publishing someone
    // else's work into the marketplace under your name is a decision, not a default.
    let inserted = sqlx::query(
        r#"
        INSERT INTO identity.templates
               (tenant_id, name, description, definition, tags, author, visibility, fork_of, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, 'tenant', $7, $8)
        RETURNING id
        "#,
    )
    .bind(tenant_id)
    .bind(&name)
    .bind(&description)
    .bind(&definition)
    .bind(&tags)
    .bind(&author)
    .bind(template_id)
    .bind(user_id)
    .fetch_one(pool)
    .await;

    // Only the unique violation is translated by hand; everything else goes through `.internal()`
    // rather than a hand-built `YorishiroError::Internal(err.into())`, which CLAUDE.md forbids.
    let row = match inserted {
        Err(err)
            if err
                .as_database_error()
                .is_some_and(|db_err| db_err.is_unique_violation()) =>
        {
            return Err(YorishiroError::Conflict {
                message: format!("this tenant already has a template named '{name}'"),
            });
        }
        other => other.internal()?,
    };

    row.try_get("id").internal()
}

/// Sets a template's marketplace visibility. Only its owning tenant may.
pub async fn set_visibility(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
    visibility: &str,
) -> Result<(), YorishiroError> {
    if !matches!(visibility, "tenant" | "community") {
        return Err(YorishiroError::ValidationFailed {
            message: format!("unknown visibility '{visibility}'"),
            details: Vec::new(),
            hint: "use 'tenant' to keep it private or 'community' to list it".into(),
        });
    }
    require_ownership(pool, tenant_id, template_id).await?;

    sqlx::query("UPDATE identity.templates SET visibility = $1, updated_at = now() WHERE id = $2")
        .bind(visibility)
        .bind(template_id)
        .execute(pool)
        .await
        .internal()?;
    Ok(())
}

/// Rejects any operation on a template the caller's tenant does not own.
///
/// Reported as NotFound rather than Forbidden: a caller that cannot act on a template should not
/// learn it exists from the difference between the two.
async fn require_ownership(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<(), YorishiroError> {
    let owned: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM identity.templates WHERE id = $1 AND tenant_id = $2")
            .bind(template_id)
            .bind(tenant_id)
            .fetch_optional(pool)
            .await
            .internal()?;

    if owned.is_none() {
        return Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/services/marketplace.rs"]
mod tests;
