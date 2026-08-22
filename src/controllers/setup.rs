//! `POST /setup` and `GET /setup/status`: first-run bootstrap, reachable without a bearer token
//! by design, same as `/auth/signup` and `/auth/login`.
//!
//! Unlike `/auth/signup`, which redeems an invite into an *existing* tenant, this creates the
//! deployment's first tenant/workspace from scratch: there is no one to invite from yet. Gated
//! on `YORISHIRO_MAX_TENANTS` resolving to an actual cap (default 1; `0` means unlimited) rather
//! than a separate flag, so the wizard can never be enabled on a deployment that lacks the
//! tenant cap that makes it safe: without that cap, anyone could hit `POST /setup` between a
//! deploy and its first real tenant and claim ownership of the whole deployment.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::{EntityTrait, PaginatorTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::embedding_provider;
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::identity_tenants;
use crate::models::identity_api_keys::IdentityApiKeys;
use crate::models::tenancy::{self, MembershipRole};

fn wizard_enabled() -> bool {
    matches!(tenancy::max_tenants_from_env(), Ok(Some(_)))
}

async fn tenant_count(conn: &impl sea_orm::ConnectionTrait) -> Result<u64, YorishiroError> {
    identity_tenants::Entity::find()
        .count(conn)
        .await
        .internal()
}

#[derive(Debug, Serialize)]
pub struct SetupStatusResponse {
    /// True when the wizard is enabled and no tenant exists yet: the client should show the
    /// setup form instead of the login form.
    pub setup_required: bool,
}

pub async fn status(State(ctx): State<AppContext>) -> Result<Json<SetupStatusResponse>, ApiError> {
    let setup_required = if wizard_enabled() {
        tenant_count(&ctx.db).await? == 0
    } else {
        false
    };
    Ok(Json(SetupStatusResponse { setup_required }))
}

#[derive(Debug, Deserialize)]
pub struct SetupRequest {
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SetupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    /// A freshly issued API key, scoped to the new owner account: shown only here, same as
    /// `/auth/login`'s, so the setup screen can log straight into the dashboard afterward.
    pub api_key: String,
}

pub async fn setup(
    State(ctx): State<AppContext>,
    Json(body): Json<SetupRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if !wizard_enabled() {
        return Err(YorishiroError::not_found(
            "the setup wizard is not enabled on this deployment",
        )
        .into());
    }

    // A fast-path check before doing any work; not the guarantee. Two concurrent POST /setup
    // calls could both pass this and both proceed, so the real check runs again after the
    // advisory lock below, inside the transaction that also does the writes.
    if tenant_count(&ctx.db).await? > 0 {
        return Err(YorishiroError::Conflict {
            message: "this deployment has already been set up".into(),
        }
        .into());
    }

    let provider = embedding_provider(&ctx)?;
    let embedding_model = crate::services::embedding::model_name_from_env();
    let dimensions = provider.dimensions() as i32;

    // tenant + workspace + user + membership run in one transaction, same reasoning as
    // `signup`'s create_user + add_member: a request that dies part-way must not leave rows
    // nothing can finish or undo. The advisory lock closes the TOCTOU window the fast-path check
    // above has, the same way `content_entities::create`'s quota check does
    // (`db::lock_for_update`): a fixed key, serializing every setup attempt on this deployment
    // against every other, so the second caller's re-check (below) sees the first caller's
    // commit before it decides whether to proceed.
    let txn = ctx.db.begin().await.internal()?;
    crate::db::lock_for_update(&txn, "setup").await.internal()?;
    if tenant_count(&txn).await? > 0 {
        return Err(YorishiroError::Conflict {
            message: "this deployment has already been set up".into(),
        }
        .into());
    }

    let tenant_active = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("default".into()),
        max_workspaces: sea_orm::ActiveValue::Set(None),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant_active, &txn)
        .await
        .internal()?;

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

pub fn routes() -> Routes {
    Routes::new()
        .prefix("setup")
        .add("/", post(setup))
        .add("/status", get(status))
}
