use sea_orm::entity::prelude::*;
use sea_orm::{ExprTrait, QuerySelect};
use serde::Serialize;

use crate::db_enum::db_enum;
use crate::error::{ResultExt, YorishiroError};
pub use crate::models::_entities::workspace_workspaces::{ActiveModel, Entity, Model};

mod tenancy;

struct AssignedEmbeddingProvider {
    provider: std::sync::Arc<dyn crate::services::embedding::EmbeddingProvider>,
    revision: i64,
}

#[loco_rs::prelude::async_trait]
impl crate::services::embedding::EmbeddingProvider for AssignedEmbeddingProvider {
    fn workspace_revision(&self) -> Option<i64> {
        Some(self.revision)
    }
    fn dimensions(&self) -> usize {
        self.provider.dimensions()
    }
    fn model_name(&self) -> String {
        self.provider.model_name()
    }
    fn availability(&self) -> crate::services::embedding::EmbeddingProviderAvailability {
        self.provider.availability()
    }
    fn count_tokens(&self, text: &str) -> u32 {
        self.provider.count_tokens(text)
    }
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        self.provider.embed_batch(texts).await
    }
    async fn embed_as(
        &self,
        kind: crate::services::embedding::EmbedKind,
        text: &str,
    ) -> Result<Vec<f32>, YorishiroError> {
        self.provider.embed_as(kind, text).await
    }
}

pub(crate) async fn check_embedding_assignment(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    provider: &dyn crate::services::embedding::EmbeddingProvider,
) -> Result<(), YorishiroError> {
    if let Some(expected) = provider.workspace_revision()
        && embedding_revision(conn, workspace_id).await? != expected
    {
        return Err(YorishiroError::Conflict {
            message: "workspace changed since provider resolution".into(),
        });
    }
    if let Some(assignment) =
        crate::models::workspace_embedding_keys::Entity::find_by_id(workspace_id)
            .one(conn)
            .await
            .internal()?
        && (assignment.model != provider.model_name()
            || usize::try_from(assignment.dimensions).ok() != Some(provider.dimensions()))
    {
        return Err(YorishiroError::Conflict {
            message: "embedding provider assignment changed; resolve the provider again".into(),
        });
    }
    Ok(())
}

/// Reads the generation fencing embedding computation and publication.
pub(crate) async fn embedding_revision(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<i64, YorishiroError> {
    Entity::find()
        .select_only()
        .column(crate::models::_entities::workspace_workspaces::Column::EmbeddingRevision)
        .filter(crate::models::_entities::workspace_workspaces::Column::Id.eq(workspace_id))
        .into_tuple::<i64>()
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("workspace {workspace_id} not found")))
}

pub(crate) async fn embedding_content_revision(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<i64, YorishiroError> {
    Entity::find()
        .select_only()
        .column(crate::models::_entities::workspace_workspaces::Column::EmbeddingContentRevision)
        .filter(crate::models::_entities::workspace_workspaces::Column::Id.eq(workspace_id))
        .into_tuple::<i64>()
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("workspace {workspace_id} not found")))
}

/// Locks the workspace generation for a short publication transaction.
pub(crate) async fn fence_embedding(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    revision: i64,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::workspace_workspaces::Column;
    use sea_orm::sea_query::Expr;
    let result = Entity::update_many()
        .col_expr(
            Column::EmbeddingRevision,
            Expr::col(Column::EmbeddingRevision),
        )
        .filter(crate::models::_entities::workspace_workspaces::Column::Id.eq(workspace_id))
        .filter(Column::EmbeddingRevision.eq(revision))
        .exec(conn)
        .await
        .internal()?;
    if result.rows_affected != 1 {
        return Err(YorishiroError::Conflict {
            message: "workspace changed while embeddings were being computed; retry the operation"
                .into(),
        });
    }
    Ok(())
}

/// Resolves the workspace override on each delivery, falling back to the process's deployment provider.
pub(crate) async fn embedding_provider(
    ctx: &loco_rs::app::AppContext,
    workspace_id: Uuid,
) -> Result<Option<std::sync::Arc<dyn crate::services::embedding::EmbeddingProvider>>, YorishiroError>
{
    let revision = embedding_revision(&ctx.db, workspace_id).await?;
    let provider = match crate::workspace_embedding_provider(&ctx.db, workspace_id).await? {
        Some(provider) => Some(provider),
        None => ctx
            .shared_store
            .get::<std::sync::Arc<dyn crate::services::embedding::EmbeddingProvider>>(),
    };
    if embedding_revision(&ctx.db, workspace_id).await? != revision {
        return Err(YorishiroError::Conflict {
            message: "workspace changed during provider resolution".into(),
        });
    }
    Ok(provider.map(|provider| {
        std::sync::Arc::new(AssignedEmbeddingProvider { provider, revision })
            as std::sync::Arc<dyn crate::services::embedding::EmbeddingProvider>
    }))
}

/// Resolves effective workspace/deployment embedding identity without constructing a provider.
pub(crate) async fn effective_embedding_metadata(
    ctx: &loco_rs::app::AppContext,
    workspace_id: Uuid,
    settings: &crate::data::settings::Settings,
) -> Result<Option<crate::services::embedding::EmbeddingMetadata>, YorishiroError> {
    if let Some(metadata) = crate::workspace_embedding_metadata(&ctx.db, workspace_id).await? {
        return Ok(Some(metadata));
    }
    crate::services::embedding::deployment_embedding_metadata(settings)
        .map_err(YorishiroError::Internal)
}

pub(crate) use tenancy::{
    delete_workspace, get_workspace, get_workspace_tenant, workspace_count_lock_key,
};

/// A workspace summary returned by tenancy lookups and signup.
#[derive(Clone, Serialize, sea_orm::FromQueryResult)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
}

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// `id` has a `uuidv7()` column default on PostgreSQL and no default on SQLite; see `crate::db::sqlite_generated_id`.
    async fn before_save<C>(mut self, db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
    }
}

#[derive(Clone, sea_orm::FromQueryResult)]
pub(crate) struct EmbeddingChainRow {
    pub(crate) embedding_model: Option<String>,
    pub(crate) embedding_dimensions: Option<i32>,
    pub(crate) tenant_model: Option<String>,
    pub(crate) tenant_dimensions: Option<i32>,
}

#[derive(Clone, sea_orm::FromQueryResult)]
pub struct StartupReindexRow {
    pub id: Uuid,
    pub embedding_model: Option<String>,
    pub embedding_dimensions: Option<i32>,
    pub embedding_reindexing: bool,
}

impl StartupReindexRow {
    /// Whether the workspace's stored vectors differ from the effective provider identity.
    /// A workspace with no stamp has no vectors to replace: its first write stamps it.
    pub fn is_stamped_with_other_embedding(&self, provider_model: &str, dimensions: usize) -> bool {
        self.embedding_reindexing
            || self.embedding_model.as_deref().is_some_and(|model| {
                model != provider_model
                    || self
                        .embedding_dimensions
                        .is_none_or(|width| usize::try_from(width).ok() != Some(dimensions))
            })
    }
}

pub(crate) async fn embedding_chain(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<EmbeddingChainRow>, YorishiroError> {
    use crate::models::_entities::tenant_tenants::Column as TenantColumn;
    use crate::models::_entities::workspace_workspaces::Column;

    Entity::find()
        .select_only()
        .column(Column::EmbeddingModel)
        .column(Column::EmbeddingDimensions)
        .column_as(TenantColumn::EmbeddingModel, "tenant_model")
        .column_as(TenantColumn::EmbeddingDimensions, "tenant_dimensions")
        .left_join(crate::models::tenant_tenants::Entity)
        .filter(Column::Id.eq(workspace_id))
        .into_model::<EmbeddingChainRow>()
        .one(conn)
        .await
        .internal()
}

pub(crate) async fn set_embedding_reindexing(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    reindexing: bool,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::workspace_workspaces::Column;
    use sea_orm::sea_query::Expr;

    Entity::update_many()
        .col_expr(Column::EmbeddingReindexing, Expr::value(reindexing))
        .filter(Column::Id.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

pub(crate) async fn embedding_reindexing(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<bool, YorishiroError> {
    use crate::models::_entities::workspace_workspaces::Column;

    let row = Entity::find()
        .select_only()
        .column(Column::EmbeddingReindexing)
        .filter(Column::Id.eq(workspace_id))
        .into_tuple::<bool>()
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("workspace {workspace_id} not found")))?;
    Ok(row)
}

pub(crate) async fn stamped_for_reindex(
    conn: &impl ConnectionTrait,
) -> Result<Vec<StartupReindexRow>, YorishiroError> {
    use crate::models::_entities::workspace_workspaces::Column;

    Entity::find()
        .select_only()
        .columns([
            Column::Id,
            Column::EmbeddingModel,
            Column::EmbeddingDimensions,
            Column::EmbeddingReindexing,
        ])
        .filter(
            Column::EmbeddingModel
                .is_not_null()
                .or(Column::EmbeddingReindexing.eq(true)),
        )
        .into_model::<StartupReindexRow>()
        .all(conn)
        .await
        .internal()
}

pub(crate) async fn list_for_actor(
    conn: &impl ConnectionTrait,
    actor: &crate::models::api_keys::AuthContext,
) -> Result<Vec<WorkspaceRecord>, YorishiroError> {
    let candidates = crate::models::tenancy::discover_workspaces(conn, actor).await?;
    Entity::find()
        .filter(
            crate::models::_entities::workspace_workspaces::Column::TenantId.eq(actor.tenant_id),
        )
        .filter(
            crate::models::_entities::workspace_workspaces::Column::Id.is_in(
                candidates
                    .into_iter()
                    .map(|workspace| workspace.workspace_id),
            ),
        )
        .all(conn)
        .await
        .internal()?
        .into_iter()
        .map(WorkspaceRecord::try_from)
        .collect()
}

pub(crate) async fn get_for_actor(
    conn: &impl ConnectionTrait,
    actor: &crate::models::api_keys::AuthContext,
    workspace_id: Uuid,
) -> Result<Model, YorishiroError> {
    let current = crate::models::api_keys::Model::current_context(conn, actor).await?;
    let workspace = get_workspace(conn, workspace_id).await?;
    let allowed = if workspace.tenant_id != current.tenant_id {
        false
    } else if let Some(user_id) = current.user_id {
        crate::models::access::workspace_access(conn, current.tenant_id, workspace_id, user_id)
            .await?
            .is_some()
    } else {
        workspace_id == current.workspace_id
    };
    if !allowed {
        return Err(YorishiroError::not_found("workspace not found"));
    }
    Ok(workspace)
}

pub(crate) async fn models_for_tenant(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<Vec<Model>, YorishiroError> {
    Entity::find()
        .filter(crate::models::_entities::workspace_workspaces::Column::TenantId.eq(tenant_id))
        .all(conn)
        .await
        .internal()
}

pub(crate) async fn stamp_embedding_if_missing(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    model: String,
    dimensions: i32,
) -> Result<(), YorishiroError> {
    use sea_orm::sea_query::Expr;

    Entity::update_many()
        .col_expr(
            crate::models::_entities::workspace_workspaces::Column::EmbeddingModel,
            Expr::value(model),
        )
        .col_expr(
            crate::models::_entities::workspace_workspaces::Column::EmbeddingDimensions,
            Expr::value(dimensions),
        )
        .filter(crate::models::_entities::workspace_workspaces::Column::Id.eq(workspace_id))
        .filter(crate::models::_entities::workspace_workspaces::Column::EmbeddingModel.is_null())
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// API-facing workspace record with a typed status.
#[derive(Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct WorkspaceRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub max_entities: Option<i32>,
    pub status: WorkspaceStatus,
    pub embedding_model: Option<String>,
    pub embedding_dimensions: Option<i32>,
    pub schema_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
}

impl TryFrom<Model> for WorkspaceRecord {
    type Error = YorishiroError;

    fn try_from(model: Model) -> Result<Self, Self::Error> {
        let status = WorkspaceStatus::from_db_str(&model.status).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown workspace status: {}",
                model.status
            ))
        })?;
        Ok(Self {
            id: model.id,
            tenant_id: model.tenant_id,
            name: model.name,
            max_entities: model.max_entities,
            status,
            embedding_model: model.embedding_model,
            embedding_dimensions: model.embedding_dimensions,
            schema_id: model.schema_id,
            created_at: model.created_at,
        })
    }
}

db_enum! {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum WorkspaceStatus {
        SchemaPending = "schema_pending",
        Active = "active",
    }
}

pub const WORKSPACE_STATUS_ACTIVE: &str = WorkspaceStatus::Active.as_db_str();

/// Whether the workspace is still waiting for its first schema.
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`, so it takes anything implementing `ConnectionTrait` (a `DatabaseTransaction`, in practice).
pub(crate) async fn is_schema_pending(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<bool, YorishiroError> {
    let status = Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .map(|model| model.status);

    Ok(status.is_some_and(|s| s == WorkspaceStatus::SchemaPending.as_db_str()))
}

/// Marks a workspace active and records its first schema, idempotently.
///
/// One statement (`COALESCE(schema_id, $new)`), not a read-then-write: two concurrent schema creations must not both see `schema_id` as `NULL` and overwrite each other's write.
///
/// This single-statement write uses `COALESCE(...)` and touches exactly the two columns allowed by `yorishiro_app`'s column-level UPDATE grant; a read followed by `ActiveModel` would overwrite concurrent changes.
pub(crate) async fn mark_active(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::workspace_workspaces::Column;
    Entity::update_many()
        .col_expr(
            Column::Status,
            Expr::value(WorkspaceStatus::Active.as_db_str()),
        )
        .col_expr(
            Column::SchemaId,
            sea_orm::sea_query::Func::coalesce([Expr::col(Column::SchemaId), Expr::val(schema_id)])
                .into(),
        )
        .filter(Column::Id.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}
