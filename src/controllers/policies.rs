//! REST routes for policies and their versions.

use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;

use crate::controllers::extractors::AuthContext;
use crate::controllers::openapi::Routes;
use crate::controllers::{ApiError, Created};
use crate::dtos::common::PolicyIdPath;
use crate::dtos::iam::ListIam;
use crate::dtos::policies::{
    CreatePolicy, CreatePolicyVersionRequest, PolicyRecord, PolicyVersionIdPath,
    PolicyVersionRecord,
};
use crate::models::pagination::ListParams;

async fn list_policies(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Query(input): Query<ListIam>,
) -> Result<Json<Vec<PolicyRecord>>, ApiError> {
    let rows =
        crate::models::iam_policies::Model::list_as(&ctx.db, &actor, ListParams::from(&input))
            .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn create_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Json(body): Json<CreatePolicy>,
) -> Result<Created<PolicyRecord>, ApiError> {
    let policy =
        crate::models::iam_policies::Model::create_as(&ctx.db, &actor, body.name, body.document)
            .await?;
    Ok(Created(policy.into()))
}

async fn get_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(PolicyIdPath { policy_id }): Path<PolicyIdPath<uuid::Uuid>>,
) -> Result<Json<PolicyRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_policies::Model::get_as(&ctx.db, &actor, policy_id)
            .await?
            .into(),
    ))
}

async fn delete_policy(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(PolicyIdPath { policy_id }): Path<PolicyIdPath<uuid::Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_policies::Model::delete_as(&ctx.db, &actor, policy_id).await?;
    Ok(axum::response::NoContent)
}

async fn list_policy_versions(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(PolicyIdPath { policy_id }): Path<PolicyIdPath<uuid::Uuid>>,
) -> Result<Json<Vec<PolicyVersionRecord>>, ApiError> {
    let versions =
        crate::models::iam_policy_versions::Model::list_as(&ctx.db, &actor, policy_id).await?;
    Ok(Json(versions.into_iter().map(Into::into).collect()))
}

async fn create_policy_version(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(PolicyIdPath { policy_id }): Path<PolicyIdPath<uuid::Uuid>>,
    Json(body): Json<CreatePolicyVersionRequest>,
) -> Result<Created<PolicyVersionRecord>, ApiError> {
    let version = crate::models::iam_policy_versions::Model::create_as(
        &ctx.db,
        &actor,
        policy_id,
        body.document,
        body.set_as_default,
    )
    .await?;
    Ok(Created(version.into()))
}

async fn get_policy_version(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(PolicyVersionIdPath {
        policy_id,
        version_id,
    }): Path<PolicyVersionIdPath>,
) -> Result<Json<PolicyVersionRecord>, ApiError> {
    Ok(Json(
        crate::models::iam_policy_versions::Model::get_as(&ctx.db, &actor, policy_id, version_id)
            .await?
            .into(),
    ))
}

async fn set_default_policy_version(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(PolicyVersionIdPath {
        policy_id,
        version_id,
    }): Path<PolicyVersionIdPath<String>>,
) -> Result<Json<PolicyVersionRecord>, ApiError> {
    let version_id = super::action_resource_id(&version_id, ":set-default")?;
    Ok(Json(
        crate::models::iam_policy_versions::Model::set_default_as(
            &ctx.db, &actor, policy_id, version_id,
        )
        .await?
        .into(),
    ))
}

async fn delete_policy_version(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(PolicyVersionIdPath {
        policy_id,
        version_id,
    }): Path<PolicyVersionIdPath>,
) -> Result<axum::response::NoContent, ApiError> {
    crate::models::iam_policy_versions::Model::delete_as(&ctx.db, &actor, policy_id, version_id)
        .await?;
    Ok(axum::response::NoContent)
}

pub(crate) fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/policies")
        .add(
            "/",
            aide::axum::routing::get_with(list_policies, |op| op.id("ListPolicies")),
        )
        .add(
            "/",
            aide::axum::routing::post_with(create_policy, |op| op.id("CreatePolicy")),
        )
        .add(
            "/{policy_id}",
            aide::axum::routing::get_with(get_policy, |op| op.id("GetPolicy")),
        )
        .add(
            "/{policy_id}",
            aide::axum::routing::delete_with(delete_policy, |op| op.id("DeletePolicy")),
        )
        .add(
            "/{policy_id}/versions",
            aide::axum::routing::get_with(list_policy_versions, |op| op.id("ListPolicyVersions")),
        )
        .add(
            "/{policy_id}/versions",
            aide::axum::routing::post_with(create_policy_version, |op| {
                op.id("CreatePolicyVersion")
            }),
        )
        .add(
            "/{policy_id}/versions/{version_id}",
            aide::axum::routing::get_with(get_policy_version, |op| op.id("GetPolicyVersion")),
        )
        .add(
            "/{policy_id}/versions/{version_id}",
            aide::axum::routing::delete_with(delete_policy_version, |op| {
                op.id("DeletePolicyVersion")
            }),
        )
        .add_action(
            "/{policy_id}/versions/{version_id}",
            ":set-default",
            aide::axum::routing::post_with(set_default_policy_version, |op| {
                op.id("SetDefaultPolicyVersion")
            }),
        )
}
