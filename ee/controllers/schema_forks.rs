//! REST endpoints for workspace-owned schema forks.

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope, Verified};
use crate::controllers::openapi::Routes;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::edition::ee::models::workspace_schema_forks as model;

async fn list(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<model::ForkRecord>>, ApiError> {
    Ok(Json(
        model::list_for_actor(&ctx, &verified.ctx, page.try_into()?).await?,
    ))
}

async fn get(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(crate::dtos::common::ForkIdPath { fork_id }): Path<crate::dtos::common::ForkIdPath<Uuid>>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        model::workspace_for_actor(&ctx.db, &verified.ctx, fork_id).await?,
    );
    let mut authorized = Authorized::<ReadScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkGet)
        .await?;
    Ok(Json(
        model::get(
            authorized.txn(),
            authorized.ctx.tenant_id,
            authorized.ctx.require_workspace()?,
            fork_id,
        )
        .await?,
    ))
}

async fn create(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Json(input): Json<model::CreateInput>,
) -> Result<(StatusCode, Json<model::ForkRecord>), ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Permission(
        crate::models::iam::policy::Action::SchemaForkCreate,
    );
    let mut authorized = Authorized::<SchemaScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkCreate)
        .await?;
    let fork = model::create(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok((StatusCode::CREATED, Json(fork)))
}

async fn update(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(crate::dtos::common::ForkIdPath { fork_id }): Path<crate::dtos::common::ForkIdPath<Uuid>>,
    Json(input): Json<model::UpdateInput>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        model::workspace_for_actor(&ctx.db, &verified.ctx, fork_id).await?,
    );
    let mut authorized = Authorized::<SchemaScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkUpdate)
        .await?;
    let fork = model::update(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        fork_id,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(fork))
}

async fn delete(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(crate::dtos::common::ForkIdPath { fork_id }): Path<crate::dtos::common::ForkIdPath<Uuid>>,
) -> Result<StatusCode, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        model::workspace_for_actor(&ctx.db, &verified.ctx, fork_id).await?,
    );
    let mut authorized = Authorized::<SchemaScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkDelete)
        .await?;
    model::delete(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        fork_id,
    )
    .await?;
    authorized.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/schema-forks")
        .add(
            "",
            aide::axum::routing::get_with(list, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["schema_fork_list"]),
                    ))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "",
            aide::axum::routing::post_with(create, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["schema"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["schema_fork_create"]),
                    ))
                    .response::<201, Json<model::ForkRecord>>()
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<422, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{fork_id}",
            aide::axum::routing::get_with(get, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["schema_fork_get"]),
                    ))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{fork_id}",
            aide::axum::routing::put_with(update, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["schema"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["schema_fork_update"]),
                    ))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<422, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{fork_id}",
            aide::axum::routing::delete_with(delete, |op| {
                op.tag("enterprise")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["schema"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["schema_fork_delete"]),
                    ))
                    .response_with::<204, (), _>(|op| op.description("Schema fork deleted"))
                    .response::<401, Json<crate::controllers::openapi::ApiErrorBody>>()
                    .response::<404, Json<crate::controllers::openapi::ApiErrorBody>>()
            }),
        )
}
