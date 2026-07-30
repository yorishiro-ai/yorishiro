use anyhow::{Context, Result, bail};
use sea_query::{Alias, Expr, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::repositories::tenancy;
use yorishiro_core::services::auth::{self, ApiKeyScope, CreatedApiKey};

#[derive(Iden)]
enum Workspaces {
    Table,
    Id,
    TenantId,
}

#[derive(Iden)]
enum ApiKeys {
    Table,
    Id,
    WorkspaceId,
    Scope,
    KeyPrefix,
    UserId,
    CreatedAt,
    LastUsedAt,
}

#[derive(Iden)]
enum Entities {
    Table,
    Id,
    WorkspaceId,
    Embedding,
}

pub async fn create_api_key(
    pool: &PgPool,
    workspace_id: Uuid,
    scope: ApiKeyScope,
    user_id: Option<Uuid>,
) -> Result<CreatedApiKey> {
    // Check the workspace exists up front so the error is clearer than a raw FK violation.
    let (sql, values) = Query::select()
        .column(Workspaces::TenantId)
        .from((Alias::new("identity"), Workspaces::Table))
        .and_where(Expr::col(Workspaces::Id).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);
    let tenant_id: Option<(Uuid,)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await?;
    let Some((tenant_id,)) = tenant_id else {
        bail!(
            "workspace '{workspace_id}' does not exist (see `admin list-workspaces <tenant-id>`)"
        );
    };

    if let Some(user_id) = user_id {
        let role = tenancy::get_membership_role(pool, tenant_id, user_id).await?;
        let Some(role) = role else {
            bail!(
                "user '{user_id}' is not a member of tenant '{tenant_id}' \
                 (see `admin add-member`)"
            );
        };
        let max_scope = role.max_scope();
        if scope > max_scope {
            bail!(
                "user '{user_id}' has role {role:?} in this tenant, which permits at most \
                 {max_scope:?} scope keys (requested {scope:?})"
            );
        }
    }

    let mut conn = pool.acquire().await?;
    let created = auth::create_api_key(&mut conn, workspace_id, scope, user_id)
        .await
        .context("failed to create api key")?;
    Ok(created)
}

#[derive(sqlx::FromRow)]
pub struct ApiKeySummary {
    pub id: Uuid,
    pub scope: String,
    pub key_prefix: String,
    pub user_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list_api_keys(pool: &PgPool, workspace_id: Uuid) -> Result<Vec<ApiKeySummary>> {
    let (sql, values) = Query::select()
        .columns([
            ApiKeys::Id,
            ApiKeys::Scope,
            ApiKeys::KeyPrefix,
            ApiKeys::UserId,
            ApiKeys::CreatedAt,
            ApiKeys::LastUsedAt,
        ])
        .from((Alias::new("identity"), ApiKeys::Table))
        .and_where(Expr::col(ApiKeys::WorkspaceId).eq(workspace_id))
        .order_by(ApiKeys::CreatedAt, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);
    let rows: Vec<ApiKeySummary> = sqlx::query_as_with(&sql, values).fetch_all(pool).await?;
    Ok(rows)
}

/// Authentication looks up the key in the database on every request, so deleting the row
/// revokes it immediately.
pub async fn revoke_api_key(pool: &PgPool, key_id: Uuid) -> Result<()> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("identity"), ApiKeys::Table))
        .and_where(Expr::col(ApiKeys::Id).eq(key_id))
        .build_sqlx(PostgresQueryBuilder);
    let result = sqlx::query_with(&sql, values).execute(pool).await?;
    if result.rows_affected() == 0 {
        bail!("api key '{key_id}' does not exist (see `admin list-api-keys <workspace-id>`)");
    }
    Ok(())
}

pub struct ResyncReport {
    pub candidates: usize,
    pub synced: usize,
    pub failed: usize,
}

/// Re-syncs embeddings for entities whose `embedding` column is still NULL. An operational
/// recovery command for entities that fell out of search due to a failed background sync
/// (e.g. a transient embedding API outage or a process killed mid-deploy).
pub async fn resync_embeddings(
    pool: &PgPool,
    workspace_id: Uuid,
    provider: &dyn yorishiro_core::services::embedding::EmbeddingProvider,
) -> Result<ResyncReport> {
    let (sql, values) = Query::select()
        .column(Entities::Id)
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(Entities::Embedding).is_null())
        .build_sqlx(PostgresQueryBuilder);
    let ids: Vec<(Uuid,)> = sqlx::query_as_with(&sql, values).fetch_all(pool).await?;

    let mut report = ResyncReport {
        candidates: ids.len(),
        synced: 0,
        failed: 0,
    };
    let mut conn = pool.acquire().await?;
    for (entity_id,) in ids {
        let result = async {
            let record =
                yorishiro_core::repositories::entities::get(&mut conn, workspace_id, entity_id)
                    .await?;
            yorishiro_core::services::embedding::sync::sync_embedding_for_record(
                &mut conn,
                workspace_id,
                &record,
                provider,
            )
            .await
        }
        .await;

        match result {
            Ok(()) => report.synced += 1,
            Err(err) => {
                report.failed += 1;
                eprintln!("  failed to resync entity {entity_id}: {err}");
            }
        }
    }
    Ok(report)
}
