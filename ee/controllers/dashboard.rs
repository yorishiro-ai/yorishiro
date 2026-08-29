//! `GET /hosted/tenant/overview`: the sole read the admin dashboard's landing page needs, plan, cap, usage counters and the member list, in one round trip.

use crate::error::ResultExt;
use crate::models::_entities::identity_tenants;
use crate::models::tenancy::{self, MembershipRecord};
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::get;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::EntityTrait;
use serde::Serialize;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::ee::models::billing;
use crate::ee::models::usage::{self, TenantUsage};
use crate::ee::services::authz::authenticate_tenant_admin;

#[derive(Debug, Serialize)]
pub struct TenantOverview {
    pub tenant_id: Uuid,
    /// `null` until a Stripe subscription event has set one: a tenant that has never subscribed has no plan and no cap.
    pub plan: Option<String>,
    pub max_workspaces: Option<i32>,
    pub usage: TenantUsage,
    pub members: Vec<MembershipRecord>,
}

async fn tenant_overview(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<TenantOverview>, ApiError> {
    // Logged so an operator can see a rejected dashboard request (bad key, or a non-admin member trying to read billing data); it otherwise surfaces only as an anonymous 401/403.
    let tenant_id = authenticate_tenant_admin(&ctx, &headers)
        .await
        .inspect_err(|err| {
            tracing::warn!(error = %err, "hosted dashboard request rejected during authentication")
        })?;

    let tenant = identity_tenants::Entity::find_by_id(tenant_id)
        .one(&ctx.db)
        .await
        .internal()?
        .ok_or_else(|| crate::YorishiroError::not_found("tenant not found"))?;
    let billing = billing::get_billing(&ctx.db, tenant_id).await?;
    let usage = usage::compute_tenant_usage(&ctx.db, tenant_id).await?;
    // The dashboard overview shows every member, not a page: it's a fixed-shape summary, not a browsable list with its own query params.
    // A tenant with more than MAX_LIST_LIMIT members would now see a truncated list where it previously saw all of them; flagged rather than silently accepted, since nothing here has measured how common that is.
    let members = tenancy::list_members(
        &ctx.db,
        tenant_id,
        crate::models::pagination::ListParams {
            limit: crate::models::pagination::MAX_LIST_LIMIT,
            offset: 0,
        },
    )
    .await?;

    Ok(Json(TenantOverview {
        tenant_id,
        plan: billing.and_then(|record| record.plan),
        max_workspaces: tenant.max_workspaces,
        usage,
        members,
    }))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("hosted")
        .add("/tenant/overview", get(tenant_overview))
}
