//! REST routes for the IAM audit events and policy simulation.

use axum::{Json, extract::State};
use loco_rs::app::AppContext;

use crate::controllers::openapi::Routes;
use crate::controllers::{ApiError, extractors::AuthContext};
use crate::dtos::iam::{
    AuditRecord, ListAudit, SimulatePrincipalPolicyRequest, SimulatePrincipalPolicyResponse,
};

async fn list_iam_audit_events(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    axum::extract::Query(input): axum::extract::Query<ListAudit>,
) -> Result<Json<Vec<AuditRecord>>, ApiError> {
    let events = crate::models::iam_audit_events::Model::list_as(
        &ctx.db,
        &actor,
        crate::models::pagination::ListParams::from(&input),
    )
    .await?;
    Ok(Json(events.into_iter().map(Into::into).collect()))
}

async fn simulate_principal_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<SimulatePrincipalPolicyRequest>,
) -> Result<Json<SimulatePrincipalPolicyResponse>, ApiError> {
    let simulation = crate::models::iam::access::simulate_principal_policy(
        &ctx.db,
        &actor,
        body.user_id,
        body.action()?,
        body.resource.as_deref(),
    )
    .await?;
    Ok(Json(simulation.into()))
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .add(
            "/api/iam-audit-events",
            aide::axum::routing::get_with(list_iam_audit_events, |op| op.id("ListIamAuditEvents")),
        )
        .add(
            "/api/simulate-principal-policy",
            aide::axum::routing::post_with(simulate_principal_policy, |op| {
                op.id("SimulatePrincipalPolicy")
            }),
        )
}
