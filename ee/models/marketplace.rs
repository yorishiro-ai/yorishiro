//! The template marketplace: `identity_templates`, `identity_template_versions` and `identity_template_reviews`.
//!
//! All three are created by the one schema migration in the root `migration` crate; this crate has never had a migration crate of its own, and the tables are base-owned even though only `ee/` reads them.
//!
//! Record shapes, input DTOs, and the reads live here.
//! A write that is a genuine decision (ownership, status validation, version numbering under lock, a rating range) stays in `services::marketplace`, which calls into this module for the insert/update itself.

use crate::error::{ResultExt, YorishiroError};
use crate::models::pagination::ListParams;
use chrono::{DateTime, Utc};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// A template as seen from the marketplace, with the aggregates a browser needs to choose one.
#[derive(Debug, Serialize, FromQueryResult)]
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

impl From<crate::models::_entities::identity_template_versions::Model> for TemplateVersionRecord {
    fn from(row: crate::models::_entities::identity_template_versions::Model) -> Self {
        Self {
            id: row.id,
            template_id: row.template_id,
            version: row.version,
            definition: row.definition,
            changelog: row.changelog,
            status: row.status,
            created_at: row.created_at.into(),
        }
    }
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

impl From<crate::models::_entities::identity_template_reviews::Model> for TemplateReviewRecord {
    fn from(row: crate::models::_entities::identity_template_reviews::Model) -> Self {
        Self {
            id: row.id,
            template_id: row.template_id,
            tenant_id: row.tenant_id,
            rating: row.rating,
            comment: row.comment,
            created_at: row.created_at.into(),
            updated_at: row.updated_at.into(),
        }
    }
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

/// The template columns a fork copies across, minus the definition (which comes from the chosen version rather than the template row).
#[derive(FromQueryResult)]
pub(crate) struct ForkSource {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) author: Option<String>,
}

/// Lists community-visible templates across every tenant, ordered by name then id, one page at a time.
///
/// A template appears only once it has a non-draft version: `visibility = 'community'` says its owner is willing to share it, but with nothing published there is nothing to install, and a listing whose every entry 404s on install is worse than a shorter one.
///
/// The three aggregates (latest stable version, review count, average rating) are correlated
/// subqueries rather than a `JOIN` + `GROUP BY`: `limit` bounds the page to 200 rows, so their cost
/// is bounded too, and nothing has measured the rewrite as faster.
///
/// That is also why the whole `SELECT` is raw SQL: three of its nine columns are those subqueries,
/// which no entity projection can produce. Putting the other six on the entity API would split one
/// query across two code paths without reducing drift, since the struct lists all nine either way.
pub async fn list_marketplace(
    conn: &impl ConnectionTrait,
    page: ListParams,
) -> Result<Vec<MarketplaceListing>, YorishiroError> {
    let limit = page.limit();
    let offset = page.offset();

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
/// **Drafts are the caller's own only.**
/// The database does not enforce this: `identity_template_versions` carries no RLS, matching `identity_templates`, so this WHERE clause is the enforcement, and dropping it publishes every tenant's unfinished work.
pub async fn list_versions(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
    page: ListParams,
) -> Result<Vec<TemplateVersionRecord>, YorishiroError> {
    use crate::models::_entities::identity_template_versions::{Column, Entity, Relation};
    use crate::models::_entities::identity_templates::Column as TemplateColumn;
    use sea_orm::{ColumnTrait, Condition, QueryFilter, QueryOrder, QuerySelect, RelationTrait};

    let rows = Entity::find()
        .filter(Column::TemplateId.eq(template_id))
        .join(
            sea_orm::JoinType::InnerJoin,
            Relation::IdentityTemplates.def(),
        )
        .filter(
            Condition::any()
                .add(TemplateColumn::TenantId.eq(tenant_id))
                .add(TemplateColumn::Visibility.eq("community")),
        )
        .filter(
            Condition::any()
                .add(Column::Status.ne("draft"))
                .add(TemplateColumn::TenantId.eq(tenant_id)),
        )
        .order_by_desc(Column::Version)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .all(conn)
        .await
        .internal()?;

    Ok(rows.into_iter().map(TemplateVersionRecord::from).collect())
}

/// Inserts the next version of a template, the number assigned as `max(version) + 1` inside the same statement.
///
/// Called inside the transaction that holds `template-version:{template_id}`'s advisory lock (`services::marketplace::publish_version`): at READ COMMITTED, Postgres locks no range for rows that do not exist yet, so two concurrent inserts would otherwise read the same maximum and collide on the `template_id, version` unique index.
///
/// Stays a hand-written `INSERT ... SELECT`, not `ActiveModel::insert`: the version number itself is computed by the same statement that inserts it, which is the concurrency guarantee above.
/// Splitting this into a separate `SELECT max(version)` plus an `ActiveModel` insert would reopen exactly the race the advisory lock and single-statement `COALESCE` together close.
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
    page: ListParams,
) -> Result<Vec<TemplateReviewRecord>, YorishiroError> {
    use crate::models::_entities::identity_template_reviews::{Column, Entity, Relation};
    use crate::models::_entities::identity_templates::Column as TemplateColumn;
    use sea_orm::{ColumnTrait, Condition, QueryFilter, QueryOrder, QuerySelect, RelationTrait};

    let rows = Entity::find()
        .filter(Column::TemplateId.eq(template_id))
        .join(
            sea_orm::JoinType::InnerJoin,
            Relation::IdentityTemplates.def(),
        )
        .filter(
            Condition::any()
                .add(TemplateColumn::TenantId.eq(tenant_id))
                .add(TemplateColumn::Visibility.eq("community")),
        )
        .order_by_desc(Column::CreatedAt)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .all(conn)
        .await
        .internal()?;

    Ok(rows.into_iter().map(TemplateReviewRecord::from).collect())
}

/// Whether a template visible to `tenant_id` exists, for `services::marketplace::submit_review`'s "reviewing a template nobody can see is meaningless" guard.
pub(crate) async fn is_visible(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<bool, YorishiroError> {
    use crate::models::_entities::identity_templates::{Column, Entity};
    use sea_orm::{ColumnTrait, Condition, PaginatorTrait, QueryFilter};

    let count = Entity::find()
        .filter(Column::Id.eq(template_id))
        .filter(
            Condition::any()
                .add(Column::TenantId.eq(tenant_id))
                .add(Column::Visibility.eq("community")),
        )
        .count(conn)
        .await
        .internal()?;
    Ok(count > 0)
}

/// Records this tenant's review, replacing its previous one if it had left one.
pub(crate) async fn upsert_review(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
    user_id: Option<Uuid>,
    request: &SubmitReviewRequest,
) -> Result<TemplateReviewRecord, YorishiroError> {
    use crate::models::_entities::identity_template_reviews::{ActiveModel, Column, Entity};

    let active = ActiveModel {
        template_id: sea_orm::ActiveValue::Set(template_id),
        tenant_id: sea_orm::ActiveValue::Set(tenant_id),
        rating: sea_orm::ActiveValue::Set(request.rating),
        comment: sea_orm::ActiveValue::Set(request.comment.clone()),
        created_by: sea_orm::ActiveValue::Set(user_id),
        updated_at: sea_orm::ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    };
    let row = Entity::insert(active)
        .on_conflict(
            OnConflict::columns([Column::TemplateId, Column::TenantId])
                .update_columns([Column::Rating, Column::Comment, Column::UpdatedAt])
                .to_owned(),
        )
        .exec_with_returning(conn)
        .await
        .internal()?;

    Ok(TemplateReviewRecord {
        id: row.id,
        template_id: row.template_id,
        tenant_id: row.tenant_id,
        rating: row.rating,
        comment: row.comment,
        created_at: row.created_at.into(),
        updated_at: row.updated_at.into(),
    })
}

/// The source template's copyable columns, if `tenant_id` may see it.
pub(crate) async fn find_fork_source(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<Option<ForkSource>, YorishiroError> {
    use crate::models::_entities::identity_templates::{Column, Entity};
    use sea_orm::{ColumnTrait, Condition, QueryFilter, QuerySelect};

    Entity::find()
        .select_only()
        .column(Column::Name)
        .column(Column::Description)
        .column(Column::Tags)
        .column(Column::Author)
        .filter(Column::Id.eq(template_id))
        .filter(
            Condition::any()
                .add(Column::TenantId.eq(tenant_id))
                .add(Column::Visibility.eq("community")),
        )
        .into_model::<ForkSource>()
        .one(conn)
        .await
        .internal()
}

/// The definition to fork: the given version if published (never a draft, even by number), or the latest `stable` one when `version` is `None`.
pub(crate) async fn find_forkable_definition(
    conn: &impl ConnectionTrait,
    template_id: Uuid,
    version: Option<i32>,
) -> Result<Option<Value>, YorishiroError> {
    use crate::models::_entities::identity_template_versions::{Column, Entity};
    use sea_orm::{ColumnTrait, QueryFilter, QueryOrder, QuerySelect};

    #[derive(FromQueryResult)]
    struct Definition {
        definition: Value,
    }

    let query = Entity::find()
        .select_only()
        .column(Column::Definition)
        .filter(Column::TemplateId.eq(template_id));

    let row = match version {
        Some(version) => {
            query
                .filter(Column::Version.eq(version))
                .filter(Column::Status.ne("draft"))
                .into_model::<Definition>()
                .one(conn)
                .await
        }
        None => {
            query
                .filter(Column::Status.eq("stable"))
                .order_by_desc(Column::Version)
                .into_model::<Definition>()
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
/// The forked copy starts `visibility = 'tenant'`: publishing someone else's work into the marketplace under your name is a decision, not a default.
pub(crate) async fn insert_fork(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    source: &ForkSource,
    definition: &Value,
    fork_of: Uuid,
    user_id: Option<Uuid>,
) -> Result<InsertForkOutcome, YorishiroError> {
    use crate::models::_entities::identity_templates::ActiveModel;

    // `visibility = 'tenant'` for the reason this function's own doc comment gives; not one of
    // ForkSource's copyable columns, since a fork never starts out community-visible regardless
    // of what its source was.
    let active = ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant_id),
        name: sea_orm::ActiveValue::Set(source.name.clone()),
        description: sea_orm::ActiveValue::Set(source.description.clone()),
        definition: sea_orm::ActiveValue::Set(definition.clone()),
        tags: sea_orm::ActiveValue::Set(source.tags.clone()),
        author: sea_orm::ActiveValue::Set(source.author.clone()),
        visibility: sea_orm::ActiveValue::Set("tenant".to_string()),
        fork_of: sea_orm::ActiveValue::Set(Some(fork_of)),
        created_by: sea_orm::ActiveValue::Set(user_id),
        ..Default::default()
    };

    match active.insert(conn).await {
        Err(sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(err)))
            if err
                .as_database_error()
                .is_some_and(|db_err| db_err.is_unique_violation()) =>
        {
            Ok(InsertForkOutcome::NameTaken)
        }
        other => Ok(InsertForkOutcome::Created(other.internal()?.id)),
    }
}

/// Whether a template exists and belongs to `tenant_id`, for `services::marketplace::require_ownership`.
pub(crate) async fn is_owned_by(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<bool, YorishiroError> {
    use crate::models::_entities::identity_templates::{Column, Entity};
    use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter};

    let count = Entity::find()
        .filter(Column::Id.eq(template_id))
        .filter(Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;
    Ok(count > 0)
}

/// Sets a template's marketplace visibility.
/// `services::marketplace::set_visibility` has already checked ownership; this is the write alone.
pub(crate) async fn update_visibility(
    conn: &impl ConnectionTrait,
    template_id: Uuid,
    visibility: &str,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::identity_templates::ActiveModel;

    // updated_at is not set here: identity_templates::ActiveModel's before_save stamps it on
    // every update whose caller didn't already set it explicitly, so a hand-written `now()` would
    // just duplicate what before_save already does.
    let active = ActiveModel {
        id: sea_orm::ActiveValue::Unchanged(template_id),
        visibility: sea_orm::ActiveValue::Set(visibility.to_string()),
        ..Default::default()
    };
    active.update(conn).await.internal()?;
    Ok(())
}
