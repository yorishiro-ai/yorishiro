use sea_orm::entity::prelude::*;
use sea_orm::{QuerySelect, Statement};
use serde::Serialize;

use crate::db_enum::db_enum;
use crate::error::{ResultExt, YorishiroError};
pub use crate::models::_entities::workspace_workspaces::{ActiveModel, Entity, Model};

#[async_trait::async_trait]
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
pub(crate) struct StartupReindexRow {
    pub(crate) id: Uuid,
    pub(crate) embedding_model: Option<String>,
}

impl StartupReindexRow {
    /// Whether the workspace's stored vectors came from a model other than `provider_model`.
    /// A workspace with no stamp has no vectors to replace: its first write stamps it.
    pub(crate) fn is_stamped_with_other_model(&self, provider_model: &str) -> bool {
        self.embedding_model
            .as_deref()
            .is_some_and(|stamped| stamped != provider_model)
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

pub(crate) async fn stamped_for_reindex(
    conn: &impl ConnectionTrait,
) -> Result<Vec<StartupReindexRow>, YorishiroError> {
    use crate::models::_entities::workspace_workspaces::Column;

    Entity::find()
        .select_only()
        .column(Column::Id)
        .column(Column::EmbeddingModel)
        .filter(Column::EmbeddingModel.is_not_null())
        .into_model::<StartupReindexRow>()
        .all(conn)
        .await
        .internal()
}

pub(crate) async fn list_for_tenant(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<Vec<WorkspaceRecord>, YorishiroError> {
    Entity::find()
        .filter(crate::models::_entities::workspace_workspaces::Column::TenantId.eq(tenant_id))
        .all(conn)
        .await
        .internal()?
        .into_iter()
        .map(WorkspaceRecord::try_from)
        .collect()
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
#[derive(Clone, Debug, Serialize)]
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
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum WorkspaceStatus {
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
/// Raw SQL, not `ActiveModel`: `COALESCE(...)` can't be expressed via `Set(...)`, and `yorishiro_app` holds UPDATE only on `workspace_workspaces (status, schema_id)` (a column-level GRANT), so the statement must touch exactly those two columns.
pub(crate) async fn mark_active(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<(), YorishiroError> {
    conn.execute_raw(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE workspace_workspaces \
         SET status = $1, schema_id = COALESCE(schema_id, $2) \
         WHERE id = $3",
        [
            WorkspaceStatus::Active.as_db_str().into(),
            schema_id.into(),
            workspace_id.into(),
        ],
    ))
    .await
    .internal()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::StartupReindexRow;

    #[test]
    fn only_a_differently_stamped_workspace_needs_a_reindex() {
        let row = |model: Option<&str>| StartupReindexRow {
            id: uuid::Uuid::nil(),
            embedding_model: model.map(str::to_owned),
        };
        assert!(row(Some("old-model")).is_stamped_with_other_model("new-model"));
        assert!(!row(Some("new-model")).is_stamped_with_other_model("new-model"));
        // An unstamped workspace has no vectors to replace.
        assert!(!row(None).is_stamped_with_other_model("new-model"));
    }
}
