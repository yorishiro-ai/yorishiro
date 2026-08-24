//! `/auth/signup` and `/auth/login`: the only two endpoints reachable without a bearer token, by design, since their entire purpose is to hand one out.
//!
//! Both run on `ctx.db` (Loco's own connection, migration role, no RLS scope), same as the admin CLI tasks: no tenant/workspace context exists yet for RLS to scope by.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::TransactionTrait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::error::{ResultExt, ValidationDetail, YorishiroError};
use crate::models::identity_api_keys::IdentityApiKeys;
use crate::models::tenancy::{self, MembershipRole, WorkspaceSummary};
use crate::services::auth::ApiKeyScope;

#[derive(Debug, Deserialize)]
pub struct SignupRequest {
    /// The plaintext token from an `admin create-invite`-issued invitation.
    /// Omit it to create a fresh tenant and join it as `Owner` instead.
    pub invite_token: Option<String>,
    /// Required when `invite_token` is omitted, rejected when it is present.
    pub email: Option<String>,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SignupResponse {
    pub user_id: Uuid,
    pub email: String,
    pub tenant_id: Uuid,
    pub role: MembershipRole,
    /// The workspaces the new member can now log into.
    /// The client picks one and passes its id to `/auth/login`.
    pub workspaces: Vec<WorkspaceSummary>,
}

pub async fn signup(
    State(ctx): State<AppContext>,
    Json(body): Json<SignupRequest>,
) -> Result<(StatusCode, Json<SignupResponse>), ApiError> {
    match body.invite_token {
        Some(ref token) => signup_with_invite(&ctx, token, &body).await,
        None => signup_without_invite(&ctx, &body).await,
    }
}

async fn signup_with_invite(
    ctx: &AppContext,
    token: &str,
    body: &SignupRequest,
) -> Result<(StatusCode, Json<SignupResponse>), ApiError> {
    if body.email.is_some() {
        return Err(YorishiroError::ValidationFailed {
            message: "email must not be given alongside invite_token".into(),
            details: vec![],
            hint: "an invite already specifies the email it was issued to".into(),
        }
        .into());
    }

    let invite = tenancy::redeem_invite(&ctx.db, token)
        .await?
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: "invite token is invalid, expired, or already used".into(),
            details: vec![],
            hint: "ask a tenant admin for a fresh invite".into(),
        })?;

    // create_user + add_member run in one transaction: see tenancy::create_user's doc comment.
    let txn = ctx.db.begin().await.internal()?;
    let user = tenancy::create_user(
        &txn,
        &invite.email,
        &body.password,
        body.display_name.as_deref(),
    )
    .await?;
    tenancy::add_member(&txn, invite.tenant_id, user.id, invite.role).await?;
    txn.commit().await.internal()?;

    let workspaces = tenancy::list_workspaces(&ctx.db, invite.tenant_id).await?;

    Ok((
        StatusCode::CREATED,
        Json(SignupResponse {
            user_id: user.id,
            email: user.email,
            tenant_id: invite.tenant_id,
            role: invite.role,
            workspaces,
        }),
    ))
}

/// No invite: creates a brand-new tenant and joins the caller as `Owner`, but no workspace.
///
/// This account cannot obtain an API key on its own: an operator must run `cargo loco task create_workspace tenant_id:<id> name:...` followed by `create_api_key`.
async fn signup_without_invite(
    ctx: &AppContext,
    body: &SignupRequest,
) -> Result<(StatusCode, Json<SignupResponse>), ApiError> {
    let email = body
        .email
        .as_deref()
        .ok_or_else(|| YorishiroError::ValidationFailed {
            message: "email is required when invite_token is omitted".into(),
            details: vec![],
            hint: "either provide invite_token, or provide email to create a new tenant".into(),
        })?;

    // tenant + user + membership run in one transaction, same reasoning as signup_with_invite's create_user + add_member: a request that dies part-way must not leave rows nothing can finish or undo.
    let tenant_name = body.display_name.as_deref().unwrap_or(email);
    let txn = ctx.db.begin().await.internal()?;
    let tenant = tenancy::create_tenant(&txn, tenant_name).await?;
    let user =
        tenancy::create_user(&txn, email, &body.password, body.display_name.as_deref()).await?;
    tenancy::add_member(&txn, tenant.id, user.id, MembershipRole::Owner).await?;
    txn.commit().await.internal()?;

    Ok((
        StatusCode::CREATED,
        Json(SignupResponse {
            user_id: user.id,
            email: user.email,
            tenant_id: tenant.id,
            role: MembershipRole::Owner,
            workspaces: vec![],
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    /// Which of the account's workspaces to issue an API key for.
    /// Omit this when the account can only reach one workspace; it resolves automatically.
    /// An account reaching more than one must specify explicitly (422 otherwise), and the refusal lists the candidates.
    pub workspace_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    /// The freshly issued API key's plaintext.
    /// Shown only in this response: only its hash is ever persisted, so it cannot be recovered afterward.
    pub api_key: String,
    pub api_key_id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    pub user_id: Uuid,
}

pub async fn login(
    State(ctx): State<AppContext>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    // Credentials are checked before the workspace is looked up, so a request with a bad password never reveals whether workspace_id exists.
    let user = tenancy::verify_login(&ctx.db, &body.email, &body.password)
        .await?
        .ok_or(YorishiroError::Unauthenticated)?;

    let workspace_id = match body.workspace_id {
        Some(workspace_id) => {
            // Confirms the workspace exists before the membership check below, matching the NotFound this call would surface anyway.
            tenancy::get_workspace_tenant(&ctx.db, workspace_id).await?;
            workspace_id
        }
        None => {
            let mut workspaces = tenancy::list_workspaces_for_user(&ctx.db, user.id).await?;
            match workspaces.len() {
                1 => workspaces.pop().expect("len() == 1 checked above").id,
                0 => {
                    return Err(YorishiroError::ScopeInsufficient {
                        message: "this account is not a member of any tenant".into(),
                        hint: "ask a tenant admin to add you as a member first".into(),
                    }
                    .into());
                }
                _ => {
                    return Err(YorishiroError::ValidationFailed {
                        message: "this account has access to more than one workspace".into(),
                        details: workspaces
                            .into_iter()
                            .map(|w| ValidationDetail {
                                field: w.id.to_string(),
                                problem: w.name,
                            })
                            .collect(),
                        hint: "specify workspace_id explicitly".into(),
                    }
                    .into());
                }
            }
        }
    };

    let tenant_id = tenancy::get_workspace_tenant(&ctx.db, workspace_id).await?;
    let role = tenancy::get_membership_role(&ctx.db, tenant_id, user.id)
        .await?
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "this account is not a member of the tenant that owns this workspace".into(),
            hint: "ask a tenant admin to add you as a member first".into(),
        })?;

    let created =
        IdentityApiKeys::create_api_key(&ctx.db, workspace_id, role.max_scope(), Some(user.id))
            .await?;

    Ok(Json(LoginResponse {
        api_key: created.plaintext,
        api_key_id: created.id,
        workspace_id: created.workspace_id,
        scope: created.scope,
        user_id: user.id,
    }))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("auth")
        .add("/signup", post(signup))
        .add("/login", post(login))
}
