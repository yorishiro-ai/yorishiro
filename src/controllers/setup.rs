//! `POST /setup` and `GET /setup/status`: first-run bootstrap, reachable without a bearer token by design, same as `/auth/signup` and `/auth/login`.
//!
//! Unlike `/auth/signup`, which redeems an invite into an *existing* tenant, this creates the deployment's first tenant/workspace from scratch: there is no one to invite from yet.
//! Gated on `settings.max_tenants` resolving to an actual cap rather than a separate flag, so the wizard can never be enabled on a deployment that lacks the tenant cap that makes it safe: without that cap, anyone could hit `POST /setup` between a deploy and its first real tenant and claim ownership of the whole deployment.
//! `0` means unlimited and disables the wizard; the environment YAML supplies the single-tenant default.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::TransactionTrait;

use crate::controllers::ApiError;
use crate::controllers::extractors::embedding_provider;
use crate::dtos::setup::{SetupRequest, SetupResponse, SetupStatusResponse};
use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::IdentityApiKeys;
use crate::models::tenancy::{self, MembershipRole};

fn max_tenants(ctx: &AppContext) -> Result<Option<i32>, YorishiroError> {
    let settings = ctx
        .shared_store
        .get::<crate::data::settings::Settings>()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("application settings missing")))?;
    Ok((settings.max_tenants > 0).then_some(settings.max_tenants))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/setup/status", responses((status = 200, body = crate::dtos::setup::SetupStatusResponse)), security(()), tag = "community"))]
pub async fn status(State(ctx): State<AppContext>) -> Result<Json<SetupStatusResponse>, ApiError> {
    let setup_required = if max_tenants(&ctx)?.is_some() {
        tenancy::count_tenants(&ctx.db).await? == 0
    } else {
        false
    };
    Ok(Json(SetupStatusResponse { setup_required }))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/setup", request_body = crate::dtos::setup::SetupRequest, responses((status = 201, body = crate::dtos::setup::SetupResponse), (status = 404, body = super::openapi::ApiErrorBody), (status = 409, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(()), tag = "community"))]
pub async fn setup(
    State(ctx): State<AppContext>,
    Json(body): Json<SetupRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let max_tenants = max_tenants(&ctx)?;
    if max_tenants.is_none() {
        return Err(YorishiroError::not_found(
            "the setup wizard is not enabled on this deployment",
        )
        .into());
    }

    // A fast-path check before doing any work, not the guarantee: two concurrent POST /setup calls could both pass this and both proceed.
    // The real check runs again after the advisory lock below, inside the transaction that also does the writes.
    if tenancy::count_tenants(&ctx.db).await? > 0 {
        return Err(YorishiroError::Conflict {
            message: "this deployment has already been set up".into(),
        }
        .into());
    }

    let provider = embedding_provider(&ctx)?;
    let embedding_model = provider.model_name();
    let dimensions = provider.dimensions() as i32;

    // tenant + workspace + user + membership run in one transaction: a request that dies part-way must not leave rows nothing can finish or undo.
    // `db::lock_for_update` closes the TOCTOU window the fast-path check above has: a fixed key, serializing every setup attempt on this deployment against every other, so the second caller's re-check (below) sees the first caller's commit before it decides whether to proceed.
    let txn = ctx.db.begin().await.internal()?;
    crate::db::lock_for_update(&txn, "setup").await.internal()?;
    if tenancy::count_tenants(&txn).await? > 0 {
        return Err(YorishiroError::Conflict {
            message: "this deployment has already been set up".into(),
        }
        .into());
    }

    let tenant = tenancy::create_tenant_with_limit(&txn, "default", max_tenants).await?;

    let workspace = tenancy::create_workspace(
        &txn,
        tenant.id,
        "default",
        None,
        None,
        Some((&embedding_model, dimensions)),
    )
    .await?;

    let user = tenancy::create_user(
        &txn,
        &body.email,
        &body.password,
        body.display_name.as_deref(),
    )
    .await?;
    tenancy::add_member(&txn, tenant.id, user.id, MembershipRole::Owner).await?;

    txn.commit().await.internal()?;

    let created = IdentityApiKeys::create_api_key(
        &ctx.db,
        workspace.id,
        MembershipRole::Owner.max_scope(),
        Some(user.id),
        false,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(SetupResponse {
            user_id: user.id,
            email: user.email,
            tenant_id: tenant.id,
            workspace_id: workspace.id,
            api_key: created.plaintext,
        }),
    ))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_status),
        super::route_inventory::path_doc(__path_setup),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("setup")
        .add("/", post(setup))
        .add("/status", get(status))
}
