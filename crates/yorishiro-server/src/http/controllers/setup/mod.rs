use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use sea_query::{Asterisk, Func, Iden, Query};
use sea_query_binder::SqlxBinder;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use yorishiro_core::db::DbHandle;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth;
use yorishiro_core::{ResultExt, YorishiroError};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Iden)]
enum Tenants {
    Table,
}

/// Whether the first-run setup wizard is enabled at all.
/// Gated on `YORISHIRO_MAX_TENANTS` resolving to an actual cap (the default is `1`; setting it to `0` means unlimited) rather than a separate flag, so the wizard can never be enabled on a deployment that lacks the tenant cap that makes it safe: without that cap, anyone could hit `POST /setup` between a deploy and its first real tenant and claim ownership of the whole deployment.
fn wizard_enabled() -> bool {
    matches!(tenancy::max_tenants_from_env(), Ok(Some(_)))
}

async fn tenant_count<C>(conn: &mut C) -> Result<i64, YorishiroError>
where
    C: yorishiro_core::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    (i64,): for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let (sql, values) = Query::select()
        .expr(Func::count(sea_query::Expr::col(Asterisk)))
        .from(C::schema_table("identity", Tenants::Table))
        .build_sqlx(C::builder());
    let (count,): (i64,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;
    Ok(count)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SetupStatusResponse {
    /// True when the wizard is enabled and no tenant exists yet: the client should show the setup form instead of the login form.
    pub setup_required: bool,
}

#[utoipa::path(
    get,
    path = "/setup/status",
    operation_id = "setup_status",
    responses(
        (status = 200, description = "Whether first-run setup should be shown", body = SetupStatusResponse),
    ),
    security(()),
    tag = "auth",
)]
pub async fn status(State(state): State<AppState>) -> Result<Json<SetupStatusResponse>, ApiError> {
    let setup_required = if wizard_enabled() {
        match &state.db {
            DbHandle::Postgres { identity, .. } => {
                let mut conn = identity.acquire().await.internal()?;
                tenant_count(&mut *conn).await? == 0
            }
            DbHandle::Sqlite(sqlite) => {
                let mut conn = yorishiro_core::db::Storage::pool(sqlite)
                    .acquire()
                    .await
                    .internal()?;
                tenant_count(&mut *conn).await? == 0
            }
        }
    } else {
        false
    };
    Ok(Json(SetupStatusResponse { setup_required }))
}

/// Unlike `/auth/signup`, which redeems an invite into an *existing* tenant, this creates the deployment's first tenant/workspace from scratch: there is no one to invite from yet.
/// Only email/password are asked for (see `web/`'s setup screen); the tenant and workspace get fixed default names, matching a self-hosted deployment's "one operator, one tenant" reality.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetupRequest {
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SetupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    /// A freshly issued API key, scoped to the new owner account: shown only here, same as `/auth/login`'s, so the setup screen can log straight into the dashboard afterward.
    pub api_key: String,
}

#[utoipa::path(
    post,
    path = "/setup",
    request_body = SetupRequest,
    responses(
        (status = 201, description = "Deployment initialized: tenant, workspace, and owner account created", body = SetupResponse),
        (status = 404, description = "The setup wizard is disabled on this deployment (YORISHIRO_MAX_TENANTS resolves to unlimited)", body = crate::error::ApiErrorBody),
        (status = 409, description = "This deployment has already been set up", body = crate::error::ApiErrorBody),
        (status = 429, description = "Too many requests from this caller; retry later"),
    ),
    security(()),
    tag = "auth",
)]
pub async fn setup(
    State(state): State<AppState>,
    Json(body): Json<SetupRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if !wizard_enabled() {
        return Err(YorishiroError::not_found(
            "the setup wizard is not enabled on this deployment",
        )
        .into());
    }

    let embedding_dimensions = state.embedding_provider.dimensions() as i32;
    let response = match &state.db {
        DbHandle::Postgres { identity, .. } => {
            let mut tx = identity.begin().await.internal()?;
            let response = setup_on(&mut *tx, &body, embedding_dimensions).await?;
            tx.commit().await.internal()?;
            response
        }
        DbHandle::Sqlite(sqlite) => {
            // No role separation to gain from a transaction across a longer-lived connection here: the identity pool and the storage pool are the same pool on this engine.
            let mut tx = yorishiro_core::db::Storage::pool(sqlite)
                .begin()
                .await
                .internal()?;
            let response = setup_on(&mut *tx, &body, embedding_dimensions).await?;
            tx.commit().await.internal()?;
            response
        }
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// The five writes `setup` makes, in one transaction on whichever engine `conn` runs.
/// Wrapped in one transaction for the same reason `signup` wraps its two: a request that dies part-way leaves rows nothing can finish or undo.
/// A tenant with no owner cannot be set up a second time, because the 409 check above sees it and refuses, and a user with no membership can never be given one, since signup expects a user that does not exist and `admin add-member` one that does.
async fn setup_on<C>(
    conn: &mut C,
    body: &SetupRequest,
    embedding_dimensions: i32,
) -> Result<SetupResponse, YorishiroError>
where
    C: yorishiro_core::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    (i64,): for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    (Uuid,): for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    tenancy::TenantRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    tenancy::WorkspaceRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
    tenancy::UserRecord: for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    if tenant_count(&mut *conn).await? > 0 {
        return Err(YorishiroError::Conflict {
            message: "this deployment has already been set up".into(),
        });
    }

    let tenant = tenancy::create_tenant_on(
        &mut *conn,
        "default",
        None,
        tenancy::max_tenants_from_env()?,
    )
    .await?;
    let workspace = tenancy::create_workspace_on(
        &mut *conn,
        tenant.id,
        "default",
        None,
        None,
        Some((&crate::embedding_model_name(), embedding_dimensions)),
    )
    .await?;

    let user = tenancy::create_user(
        &mut *conn,
        &body.email,
        &body.password,
        body.display_name.as_deref(),
    )
    .await?;
    tenancy::add_member(&mut *conn, tenant.id, user.id, MembershipRole::Owner).await?;

    let created = auth::create_api_key(
        &mut *conn,
        workspace.id,
        MembershipRole::Owner.max_scope(),
        Some(user.id),
    )
    .await?;

    Ok(SetupResponse {
        user_id: user.id,
        email: user.email,
        tenant_id: tenant.id,
        workspace_id: workspace.id,
        api_key: created.plaintext,
    })
}

#[cfg(test)]
#[path = "../../../../tests/http/controllers/setup/mod.rs"]
mod tests;
