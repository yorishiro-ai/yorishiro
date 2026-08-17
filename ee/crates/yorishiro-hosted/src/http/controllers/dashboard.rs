use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::Serialize;
use uuid::Uuid;
use yorishiro_core::ResultExt;
use yorishiro_core::repositories::tenancy::{self, MembershipRecord};

use crate::error::HostedApiError;
use crate::services::authz::authenticate_tenant_admin;
use crate::services::billing;
use crate::services::usage::{self, TenantUsage};
use crate::state::HostedState;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TenantOverview {
    pub tenant_id: Uuid,
    /// `null` until a Stripe subscription event has set one: a tenant that has never subscribed has no plan and no cap.
    pub plan: Option<String>,
    pub max_workspaces: Option<i32>,
    pub usage: TenantUsage,
    pub members: Vec<MembershipRecord>,
}

/// `GET /hosted/tenant/overview` is the sole read the dashboard's landing page needs: plan, cap, usage counters, and the member list, in one round trip.
#[utoipa::path(
    get,
    path = "/hosted/tenant/overview",
    responses(
        (status = 200, description = "Plan, workspace cap, usage counters and the member list", body = TenantOverview),
        (status = 401, description = "Missing or invalid bearer key", body = crate::error::HostedApiErrorBody),
        (status = 403, description = "Key's tenant membership is not owner/admin: billing and usage are a tenant-admin concern regardless of the key's own scope", body = crate::error::HostedApiErrorBody),
    ),
    security(("bearer_key" = [])),
    tag = "hosted",
)]
pub async fn tenant_overview(
    State(state): State<HostedState>,
    headers: HeaderMap,
) -> Result<Json<TenantOverview>, HostedApiError> {
    // Logged so an operator can see a rejected dashboard request (bad key, or a non-admin member trying to read billing data); it otherwise surfaces only as an anonymous 401/403.
    let tenant_id = authenticate_tenant_admin(&state, &headers)
        .await
        .inspect_err(|err| {
            tracing::warn!(error = %err, "hosted dashboard request rejected during authentication")
        })?;

    let mut conn = state.identity_pool.acquire().await.internal()?;
    let tenant = tenancy::get_tenant(&mut conn, tenant_id).await?;
    let billing = billing::get_billing(&state.identity_pool, tenant_id).await?;
    let usage = usage::compute_tenant_usage(&state.identity_pool, tenant_id).await?;
    let members = tenancy::list_members(&state.identity_pool, tenant_id).await?;

    Ok(Json(TenantOverview {
        tenant_id,
        plan: billing.and_then(|record| record.plan),
        max_workspaces: tenant.max_workspaces,
        usage,
        members,
    }))
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/dashboard.rs"]
mod tests;
