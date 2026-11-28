//! The template marketplace: `template_templates`, `template_versions` and `template_reviews`.
//!
//! The tables belong to the community schema migration even though only `ee/` reads them.
//!
//! Record shapes, input DTOs and reads live here.
//! Write decisions live in the private `write` module.

mod write;

pub(crate) use write::{fork_template, publish_version, set_visibility, submit_review};

use crate::error::OptionExt;
use sea_orm::SelectExt;

use crate::error::{ResultExt, YorishiroError};
use crate::models::pagination::ListParams;
use crate::models::template_templates::TemplateVisibility;
use chrono::{DateTime, Utc};
use sea_orm::ExprTrait;
use sea_orm::sea_query::{Expr, OnConflict, Query};
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, FromQueryResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

async fn authorized_transaction(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    action: crate::models::iam::policy::Action,
) -> Result<sea_orm::DatabaseTransaction, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        action,
        crate::models::iam::policy::Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    Ok(txn)
}

pub(crate) async fn list_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    page: ListParams,
) -> Result<Vec<MarketplaceListing>, YorishiroError> {
    let txn = authorized_transaction(
        db,
        actor,
        crate::models::iam::policy::Action::ListMarketplaceTemplates,
    )
    .await?;
    list_marketplace(&txn, page).await
}

pub(crate) async fn versions_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    template_id: Uuid,
    page: ListParams,
) -> Result<Vec<TemplateVersionRecord>, YorishiroError> {
    let txn = authorized_transaction(
        db,
        actor,
        crate::models::iam::policy::Action::ListMarketplaceVersions,
    )
    .await?;
    list_versions(&txn, actor.tenant_id, template_id, page).await
}

pub(crate) async fn get_template_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    template_id: Uuid,
) -> Result<MarketplaceListing, YorishiroError> {
    let txn = authorized_transaction(
        db,
        actor,
        crate::models::iam::policy::Action::GetMarketplaceTemplate,
    )
    .await?;
    marketplace_listings(&txn, Some(template_id), ListParams::default())
        .await?
        .pop()
        .or_not_found("marketplace template was not found")
}

pub(crate) async fn get_version_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    template_id: Uuid,
    version_id: Uuid,
) -> Result<TemplateVersionRecord, YorishiroError> {
    let txn = authorized_transaction(
        db,
        actor,
        crate::models::iam::policy::Action::GetMarketplaceVersion,
    )
    .await?;
    require_postgres(&txn, "reading a marketplace version")?;
    visible_versions(
        &txn,
        actor.tenant_id,
        template_id,
        Some(version_id),
        ListParams::default(),
    )
    .await?
    .pop()
    .or_not_found("marketplace version was not found")
}

pub(crate) async fn reviews_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    template_id: Uuid,
    page: ListParams,
) -> Result<Vec<TemplateReviewRecord>, YorishiroError> {
    let txn = authorized_transaction(
        db,
        actor,
        crate::models::iam::policy::Action::ListMarketplaceReviews,
    )
    .await?;
    list_reviews(&txn, actor.tenant_id, template_id, page).await
}

/// The marketplace spans tenants, and SQLite holds one, so its cross-tenant queries are written for PostgreSQL only.
/// Refusing here answers with `501` instead of a query failure.
fn require_postgres(conn: &impl ConnectionTrait, what: &str) -> Result<(), YorishiroError> {
    if crate::db::is_postgres(conn) {
        return Ok(());
    }
    Err(YorishiroError::BackendUnsupported {
        message: format!("{what} requires PostgreSQL"),
    })
}

/// A template as seen from the marketplace, with the aggregates a browser needs to choose one.
#[derive(Serialize, FromQueryResult, schemars::JsonSchema)]
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

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct TemplateVersionRecord {
    #[serde(rename = "version_id")]
    pub id: Uuid,
    pub template_id: Uuid,
    pub version: i32,
    #[schemars(with = "serde_json::Value")]
    pub definition: Value,
    pub changelog: Option<String>,
    pub status: TemplateVersionStatus,
    pub created_at: DateTime<Utc>,
}

crate::models::db_enum::db_enum! {
    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    pub enum TemplateVersionStatus {
        Draft = "draft",
        Pre = "pre",
        Stable = "stable",
    }
}

impl TemplateVersionStatus {
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub fn parse_publish(value: &str) -> Result<Self, YorishiroError> {
        Self::from_db_str(value).ok_or_else(|| YorishiroError::ValidationFailed {
            message: format!("unknown publish status '{value}'"),
            details: Vec::new(),
            hint: "use one of: draft, pre, stable".into(),
        })
    }
}

#[derive(FromQueryResult)]
struct MarketplaceListingDbRecord {
    template_id: Uuid,
    name: String,
    description: Option<String>,
    tags: Value,
    author: Option<String>,
    tenant_id: Uuid,
    latest_stable_version: Option<i32>,
    review_count: i64,
    average_rating: Option<f64>,
}

#[derive(FromQueryResult)]
struct TemplateVersionDbRecord {
    id: Uuid,
    template_id: Uuid,
    version: i32,
    definition: Value,
    changelog: Option<String>,
    status: String,
    created_at: DateTime<Utc>,
}

#[derive(FromQueryResult)]
struct AuthorizedTemplateVersionDbRecord {
    // Visibility is projected only after the query's authorization predicate.
    // An inaccessible corrupt row must remain indistinguishable from a missing row.
    id: Uuid,
    template_id: Uuid,
    version: i32,
    definition: Value,
    changelog: Option<String>,
    status: String,
    created_at: DateTime<Utc>,
    template_visibility: String,
}

impl TryFrom<AuthorizedTemplateVersionDbRecord> for TemplateVersionRecord {
    type Error = YorishiroError;

    fn try_from(row: AuthorizedTemplateVersionDbRecord) -> Result<Self, Self::Error> {
        TemplateVisibility::from_db_str(&row.template_visibility).ok_or_else(|| {
            YorishiroError::Internal(
                format!("unknown template visibility: {}", row.template_visibility).into(),
            )
        })?;
        TemplateVersionDbRecord {
            id: row.id,
            template_id: row.template_id,
            version: row.version,
            definition: row.definition,
            changelog: row.changelog,
            status: row.status,
            created_at: row.created_at,
        }
        .try_into()
    }
}

impl TryFrom<TemplateVersionDbRecord> for TemplateVersionRecord {
    type Error = YorishiroError;

    fn try_from(row: TemplateVersionDbRecord) -> Result<Self, Self::Error> {
        let status = TemplateVersionStatus::from_db_str(&row.status).ok_or_else(|| {
            YorishiroError::Internal(
                format!("unknown template version status: {}", row.status).into(),
            )
        })?;
        Ok(Self {
            id: row.id,
            template_id: row.template_id,
            version: row.version,
            definition: row.definition,
            changelog: row.changelog,
            status,
            created_at: row.created_at,
        })
    }
}

impl TryFrom<crate::models::_entities::template_versions::Model> for TemplateVersionRecord {
    type Error = YorishiroError;

    fn try_from(
        row: crate::models::_entities::template_versions::Model,
    ) -> Result<Self, Self::Error> {
        TemplateVersionDbRecord {
            id: row.id,
            template_id: row.template_id,
            version: row.version,
            definition: row.definition,
            changelog: row.changelog,
            status: row.status,
            created_at: row.created_at.into(),
        }
        .try_into()
    }
}

#[derive(Serialize, FromQueryResult, schemars::JsonSchema)]
pub struct TemplateReviewRecord {
    #[serde(rename = "review_id")]
    pub id: Uuid,
    pub template_id: Uuid,
    pub tenant_id: Uuid,
    pub rating: i16,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<crate::models::_entities::template_reviews::Model> for TemplateReviewRecord {
    fn from(row: crate::models::_entities::template_reviews::Model) -> Self {
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

#[derive(FromQueryResult)]
struct AuthorizedTemplateReviewDbRecord {
    id: Uuid,
    template_id: Uuid,
    tenant_id: Uuid,
    rating: i16,
    comment: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    template_visibility: String,
}

impl TryFrom<AuthorizedTemplateReviewDbRecord> for TemplateReviewRecord {
    type Error = YorishiroError;

    fn try_from(row: AuthorizedTemplateReviewDbRecord) -> Result<Self, Self::Error> {
        TemplateVisibility::from_db_str(&row.template_visibility).ok_or_else(|| {
            YorishiroError::Internal(
                format!("unknown template visibility: {}", row.template_visibility).into(),
            )
        })?;
        Ok(Self {
            id: row.id,
            template_id: row.template_id,
            tenant_id: row.tenant_id,
            rating: row.rating,
            comment: row.comment,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(Deserialize)]
pub struct PublishVersionRequest {
    pub definition: Value,
    pub changelog: Option<String>,
    /// `draft` (default), `pre`, or `stable`.
    #[serde(default = "default_status")]
    pub status: TemplateVersionStatus,
}

const fn default_status() -> TemplateVersionStatus {
    TemplateVersionStatus::Draft
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SubmitReviewRequest {
    pub rating: i16,
    pub comment: Option<String>,
}

/// The template columns a fork copies across, minus the definition (which comes from the chosen version rather than the template row).
pub(crate) struct ForkSource {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) author: Option<String>,
}

#[derive(FromQueryResult)]
struct ForkSourceDbRecord {
    name: String,
    description: Option<String>,
    tags: Value,
    author: Option<String>,
    template_visibility: String,
}

impl TryFrom<ForkSourceDbRecord> for ForkSource {
    type Error = YorishiroError;

    fn try_from(row: ForkSourceDbRecord) -> Result<Self, Self::Error> {
        TemplateVisibility::from_db_str(&row.template_visibility).ok_or_else(|| {
            YorishiroError::Internal(
                format!("unknown template visibility: {}", row.template_visibility).into(),
            )
        })?;
        Ok(Self {
            name: row.name,
            description: row.description,
            tags: crate::models::template_templates::tags_from_json(row.tags)?,
            author: row.author,
        })
    }
}

/// Lists community-visible templates across every tenant, ordered by name then id, one page at a time.
///
/// A template appears only once it has a non-draft version, because with nothing published there is nothing to install.
///
/// The three aggregates (latest stable version, review count, average rating) are correlated subqueries, and the page limit bounds their cost.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn list_marketplace(
    conn: &impl ConnectionTrait,
    page: ListParams,
) -> Result<Vec<MarketplaceListing>, YorishiroError> {
    marketplace_listings(conn, None, page).await
}

async fn marketplace_listings(
    conn: &impl ConnectionTrait,
    template_id: Option<Uuid>,
    page: ListParams,
) -> Result<Vec<MarketplaceListing>, YorishiroError> {
    use crate::models::_entities::{
        template_reviews as review, template_templates as template, template_versions as version,
    };
    use sea_orm::sea_query::{Alias, SimpleExpr};
    use sea_orm::{ColumnTrait, QueryFilter, QueryOrder, QuerySelect, QueryTrait};
    require_postgres(conn, "listing the marketplace")?;
    let latest = Query::select()
        .expr(Expr::col((version::Entity, version::Column::Version)).max())
        .from(version::Entity)
        .and_where(
            Expr::col((version::Entity, version::Column::TemplateId))
                .equals((template::Entity, template::Column::Id)),
        )
        .and_where(Expr::col((version::Entity, version::Column::Status)).eq("stable"))
        .to_owned();
    let review_count = Query::select()
        .expr(Expr::col((review::Entity, review::Column::Id)).count())
        .from(review::Entity)
        .and_where(
            Expr::col((review::Entity, review::Column::TemplateId))
                .equals((template::Entity, template::Column::Id)),
        )
        .to_owned();
    let average = Query::select()
        .expr(
            Expr::col((review::Entity, review::Column::Rating))
                .avg()
                .cast_as(Alias::new("float8")),
        )
        .from(review::Entity)
        .and_where(
            Expr::col((review::Entity, review::Column::TemplateId))
                .equals((template::Entity, template::Column::Id)),
        )
        .to_owned();
    let published = Query::select()
        .expr(Expr::val(1))
        .from(version::Entity)
        .and_where(
            Expr::col((version::Entity, version::Column::TemplateId))
                .equals((template::Entity, template::Column::Id)),
        )
        .and_where(Expr::col((version::Entity, version::Column::Status)).is_in(["pre", "stable"]))
        .to_owned();
    let rows = template::Entity::find()
        .select_only()
        .column_as(template::Column::Id, "template_id")
        .columns([
            template::Column::Name,
            template::Column::Description,
            template::Column::Tags,
            template::Column::Author,
            template::Column::TenantId,
        ])
        .expr_as(
            SimpleExpr::SubQuery(None, Box::new(latest.into())),
            "latest_stable_version",
        )
        .expr_as(
            SimpleExpr::SubQuery(None, Box::new(review_count.into())),
            "review_count",
        )
        .expr_as(
            SimpleExpr::SubQuery(None, Box::new(average.into())),
            "average_rating",
        )
        .filter(template::Column::Visibility.eq(TemplateVisibility::Community.as_db_str()))
        .filter(Expr::exists(published))
        .apply_if(template_id, |query, id| {
            query.filter(template::Column::Id.eq(id))
        })
        .order_by_asc(template::Column::Name)
        .order_by_asc(template::Column::Id)
        .limit(page.limit())
        .offset(page.offset())
        .into_model::<MarketplaceListingDbRecord>()
        .all(conn)
        .await
        .internal()?;
    rows.into_iter()
        .map(|row| {
            Ok(MarketplaceListing {
                template_id: row.template_id,
                name: row.name,
                description: row.description,
                tags: serde_json::from_value(row.tags).internal()?,
                author: row.author,
                tenant_id: row.tenant_id,
                latest_stable_version: row.latest_stable_version,
                review_count: row.review_count,
                average_rating: row.average_rating,
            })
        })
        .collect()
}

/// Versions of a template that `tenant_id` is allowed to see.
///
/// **Drafts are the caller's own only.**
/// `template_versions` carries no RLS, so this WHERE clause is the only enforcement.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn list_versions(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
    page: ListParams,
) -> Result<Vec<TemplateVersionRecord>, YorishiroError> {
    visible_versions(conn, tenant_id, template_id, None, page).await
}

async fn visible_versions(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
    version_id: Option<Uuid>,
    page: ListParams,
) -> Result<Vec<TemplateVersionRecord>, YorishiroError> {
    use crate::models::_entities::template_templates::Column as TemplateColumn;
    use crate::models::_entities::template_versions::{Column, Entity, Relation};
    use sea_orm::{
        ColumnTrait, Condition, QueryFilter, QueryOrder, QuerySelect, QueryTrait, RelationTrait,
    };

    let rows = Entity::find()
        .select_only()
        .column(Column::Id)
        .column(Column::TemplateId)
        .column(Column::Version)
        .column(Column::Definition)
        .column(Column::Changelog)
        .column(Column::Status)
        .column(Column::CreatedAt)
        .column_as(TemplateColumn::Visibility, "template_visibility")
        .filter(Column::TemplateId.eq(template_id))
        .apply_if(version_id, |query, id| query.filter(Column::Id.eq(id)))
        .join(
            sea_orm::JoinType::InnerJoin,
            Relation::TemplateTemplates.def(),
        )
        .filter(
            Condition::any()
                .add(TemplateColumn::TenantId.eq(tenant_id))
                .add(TemplateColumn::Visibility.eq(TemplateVisibility::Community.as_db_str())),
        )
        .filter(
            Condition::any()
                .add(Column::Status.ne("draft"))
                .add(TemplateColumn::TenantId.eq(tenant_id)),
        )
        .order_by_desc(Column::Version)
        .limit(page.limit())
        .offset(page.offset())
        .into_model::<AuthorizedTemplateVersionDbRecord>()
        .all(conn)
        .await
        .internal()?;

    rows.into_iter()
        .map(TemplateVersionRecord::try_from)
        .collect()
}

/// Inserts the next version of a template, the number assigned as `max(version) + 1` inside the same statement.
///
/// Call it inside a transaction holding the `template-version:{template_id}` advisory lock.
/// At READ COMMITTED, PostgreSQL locks no range for rows that do not exist yet, so concurrent inserts would otherwise read the same maximum.
///
/// One `INSERT ... SELECT` computes the version in the inserting statement, so do not split it into a separate `SELECT max(version)`.
pub(crate) async fn insert_next_version(
    conn: &impl ConnectionTrait,
    template_id: Uuid,
    request: &PublishVersionRequest,
    user_id: Option<Uuid>,
) -> Result<TemplateVersionRecord, YorishiroError> {
    use crate::models::_entities::template_versions::{Column, Entity};

    require_postgres(conn, "publishing a template version")?;
    let select = Query::select()
        .expr(Expr::val(Uuid::now_v7()))
        .expr(Expr::val(template_id))
        .expr(Expr::col(Column::Version).max().if_null(0).add(1))
        .expr(Expr::val(request.definition.clone()))
        .expr(Expr::val(request.changelog.clone()))
        .expr(Expr::val(request.status.as_db_str()))
        .expr(Expr::val(user_id))
        .expr(Expr::val(crate::data::time::now()))
        .from(Entity)
        .and_where(Expr::col(Column::TemplateId).eq(template_id))
        .to_owned();
    let statement = Query::insert()
        .into_table(Entity)
        .columns([
            Column::Id,
            Column::TemplateId,
            Column::Version,
            Column::Definition,
            Column::Changelog,
            Column::Status,
            Column::CreatedBy,
            Column::CreatedAt,
        ])
        .select_from(select)
        .internal()?
        .returning(Query::returning().columns([
            Column::Id,
            Column::TemplateId,
            Column::Version,
            Column::Definition,
            Column::Changelog,
            Column::Status,
            Column::CreatedAt,
        ]))
        .to_owned();
    TemplateVersionDbRecord::find_by_statement(conn.get_database_backend().build(&statement))
        .one(conn)
        .await
        .internal()?
        .or_internal("insert did not return a row")
        .and_then(TemplateVersionRecord::try_from)
}

/// Reviews of a template, readable by anyone who can see the template itself.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn list_reviews(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
    page: ListParams,
) -> Result<Vec<TemplateReviewRecord>, YorishiroError> {
    use crate::models::_entities::template_reviews::{Column, Entity, Relation};
    use crate::models::_entities::template_templates::Column as TemplateColumn;
    use sea_orm::{ColumnTrait, Condition, QueryFilter, QueryOrder, QuerySelect, RelationTrait};

    let rows = Entity::find()
        .select_only()
        .column(Column::Id)
        .column(Column::TemplateId)
        .column(Column::TenantId)
        .column(Column::Rating)
        .column(Column::Comment)
        .column(Column::CreatedAt)
        .column(Column::UpdatedAt)
        .column_as(TemplateColumn::Visibility, "template_visibility")
        .filter(Column::TemplateId.eq(template_id))
        .join(
            sea_orm::JoinType::InnerJoin,
            Relation::TemplateTemplates.def(),
        )
        .filter(
            Condition::any()
                .add(TemplateColumn::TenantId.eq(tenant_id))
                .add(TemplateColumn::Visibility.eq(TemplateVisibility::Community.as_db_str())),
        )
        .order_by_desc(Column::CreatedAt)
        .limit(page.limit())
        .offset(page.offset())
        .into_model::<AuthorizedTemplateReviewDbRecord>()
        .all(conn)
        .await
        .internal()?;

    rows.into_iter()
        .map(TemplateReviewRecord::try_from)
        .collect()
}

/// Whether a template visible to `tenant_id` exists.
pub(crate) async fn is_visible(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<bool, YorishiroError> {
    use crate::models::_entities::template_templates::{Column, Entity};
    use sea_orm::{ColumnTrait, Condition, QueryFilter};

    Entity::find()
        .filter(Column::Id.eq(template_id))
        .filter(
            Condition::any()
                .add(Column::TenantId.eq(tenant_id))
                .add(Column::Visibility.eq(TemplateVisibility::Community.as_db_str())),
        )
        .exists(conn)
        .await
        .internal()
}

/// Records this tenant's review, replacing its previous one if it had left one.
pub(crate) async fn upsert_review(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
    user_id: Option<Uuid>,
    request: &SubmitReviewRequest,
) -> Result<TemplateReviewRecord, YorishiroError> {
    use crate::models::_entities::template_reviews::{ActiveModel, Column, Entity};

    let active = ActiveModel {
        template_id: sea_orm::ActiveValue::Set(template_id),
        tenant_id: sea_orm::ActiveValue::Set(tenant_id),
        rating: sea_orm::ActiveValue::Set(request.rating),
        comment: sea_orm::ActiveValue::Set(request.comment.clone()),
        created_by: sea_orm::ActiveValue::Set(user_id),
        updated_at: sea_orm::ActiveValue::Set(crate::data::time::now().into()),
        ..Default::default()
    };
    let active = sea_orm::ActiveModelBehavior::before_save(active, conn, true)
        .await
        .internal()?;
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
    use crate::models::_entities::template_templates::{Column, Entity};
    use sea_orm::{ColumnTrait, Condition, QueryFilter, QuerySelect};

    Entity::find()
        .select_only()
        .column(Column::Name)
        .column(Column::Description)
        .column(Column::Tags)
        .column(Column::Author)
        .column_as(Column::Visibility, "template_visibility")
        .filter(Column::Id.eq(template_id))
        .filter(
            Condition::any()
                .add(Column::TenantId.eq(tenant_id))
                .add(Column::Visibility.eq(TemplateVisibility::Community.as_db_str())),
        )
        .into_model::<ForkSourceDbRecord>()
        .one(conn)
        .await
        .internal()?
        .map(ForkSource::try_from)
        .transpose()
}

/// The definition to fork: the given version if published (never a draft, even by number), or the latest `stable` one when `version` is `None`.
pub(crate) async fn find_forkable_definition(
    conn: &impl ConnectionTrait,
    template_id: Uuid,
    version: Option<i32>,
) -> Result<Option<Value>, YorishiroError> {
    use crate::models::_entities::template_versions::{Column, Entity};
    use sea_orm::{ColumnTrait, QueryFilter, QueryOrder, QuerySelect};

    #[derive(FromQueryResult)]
    struct Definition {
        definition: Value,
        status: String,
    }

    let row = match version {
        Some(version) => Entity::find()
            .select_only()
            .column(Column::Definition)
            .column(Column::Status)
            .filter(Column::TemplateId.eq(template_id))
            .filter(Column::Version.eq(version))
            .into_model::<Definition>()
            .one(conn)
            .await
            .internal()?,
        None => Entity::find()
            .select_only()
            .column(Column::Definition)
            .column(Column::Status)
            .filter(Column::TemplateId.eq(template_id))
            .filter(Column::Status.eq(TemplateVersionStatus::Stable.as_db_str()))
            .order_by_desc(Column::Version)
            .into_model::<Definition>()
            .one(conn)
            .await
            .internal()?,
    };

    let Some(row) = row else {
        return Ok(None);
    };
    let status = TemplateVersionStatus::from_db_str(&row.status).ok_or_else(|| {
        YorishiroError::Internal(format!("unknown template version status: {}", row.status).into())
    })?;

    Ok((version.is_none() || status != TemplateVersionStatus::Draft).then_some(row.definition))
}

/// Outcome of [`insert_fork`]: either the new template's id, or the name it collided on.
pub(crate) enum InsertForkOutcome {
    Created(Uuid),
    NameTaken,
}

/// Inserts the caller's own private copy of a forked template.
/// The copy starts tenant-private, whatever the source's visibility.
pub(crate) async fn insert_fork(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    source: &ForkSource,
    definition: &Value,
    fork_of: Uuid,
    user_id: Option<Uuid>,
) -> Result<InsertForkOutcome, YorishiroError> {
    use crate::models::_entities::template_templates::ActiveModel;

    let active = ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant_id),
        name: sea_orm::ActiveValue::Set(source.name.clone()),
        description: sea_orm::ActiveValue::Set(source.description.clone()),
        definition: sea_orm::ActiveValue::Set(definition.clone()),
        tags: sea_orm::ActiveValue::Set(crate::models::template_templates::tags_to_json(
            &source.tags,
        )),
        author: sea_orm::ActiveValue::Set(source.author.clone()),
        visibility: sea_orm::ActiveValue::Set(TemplateVisibility::Tenant.as_db_str().to_string()),
        fork_of: sea_orm::ActiveValue::Set(Some(fork_of)),
        created_by: sea_orm::ActiveValue::Set(user_id),
        ..Default::default()
    };

    match active.insert(conn).await {
        Err(sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(err)))
            if err
                .as_database_error()
                .is_some_and(sea_orm::sqlx::error::DatabaseError::is_unique_violation) =>
        {
            Ok(InsertForkOutcome::NameTaken)
        }
        other => Ok(InsertForkOutcome::Created(other.internal()?.id)),
    }
}

/// Whether a template exists and belongs to `tenant_id`.
pub(crate) async fn is_owned_by(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<bool, YorishiroError> {
    use crate::models::_entities::template_templates::{Column, Entity};
    use sea_orm::{ColumnTrait, QueryFilter};

    Entity::find()
        .filter(Column::Id.eq(template_id))
        .filter(Column::TenantId.eq(tenant_id))
        .exists(conn)
        .await
        .internal()
}

/// Writes a template's marketplace visibility; the caller has already checked ownership.
pub(crate) async fn update_visibility(
    conn: &impl ConnectionTrait,
    template_id: Uuid,
    visibility: TemplateVisibility,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::template_templates::ActiveModel;

    let active = ActiveModel {
        id: sea_orm::ActiveValue::Unchanged(template_id),
        visibility: sea_orm::ActiveValue::Set(visibility.as_db_str().to_string()),
        ..Default::default()
    };
    active.update(conn).await.map_err(|error| {
        YorishiroError::not_found_on_missing(error, "template version was not found")
    })?;
    Ok(())
}
