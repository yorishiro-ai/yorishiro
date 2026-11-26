//! `POST /setup` and `GET /setup/status`: first-run bootstrap, reachable without a bearer token by design, same as `/auth/signup` and `/auth/root/login`.
//!
//! Creates the deployment's first tenant and admin account, then issues an admin access key.
//! The admin creates and selects workspaces explicitly.
//! The resolved edition tenant cap controls whether first-run setup is available.
//! An unlimited cap disables the wizard; the source default is one tenant.

use crate::controllers::Created;
use crate::error::OptionExt;
use axum::Json;
use axum::extract::State;
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::openapi::Routes;
use crate::dtos::setup::{SetupRequest, SetupResponse, SetupStatusResponse};
use crate::error::YorishiroError;
use crate::models::{tenancy, tenant_tenants};

fn max_tenants(ctx: &AppContext) -> Result<Option<i32>, YorishiroError> {
    ctx.shared_store
        .get::<crate::data::tenant_policy::TenantCap>()
        .map(|cap| cap.resolve())
        .or_internal("tenant cap policy missing")
}

async fn status(State(ctx): State<AppContext>) -> Result<Json<SetupStatusResponse>, ApiError> {
    let setup_required = if max_tenants(&ctx)?.is_some() {
        tenant_tenants::count_tenants(&ctx.db).await? == 0
    } else {
        false
    };
    Ok(Json(SetupStatusResponse { setup_required }))
}

async fn setup(
    State(ctx): State<AppContext>,
    Json(body): Json<SetupRequest>,
) -> Result<Created<SetupResponse>, ApiError> {
    let account = tenancy::setup_first_account(
        &ctx.db,
        max_tenants(&ctx)?,
        &body.email,
        &body.password,
        body.display_name.as_deref(),
    )
    .await?;
    let (created, _) =
        tenancy::issue_login_key(&ctx.db, account.user_id, None, Some(account.tenant_id)).await?;

    Ok(Created(SetupResponse {
        user_id: account.user_id,
        email: account.email,
        tenant_id: account.tenant_id,
        access_key: created.plaintext,
    }))
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("setup")
        .add("/", aide::axum::routing::post(setup))
        .add("/status", aide::axum::routing::get(status))
}
