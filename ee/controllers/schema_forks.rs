//! REST endpoints for workspace-owned schema forks.

use crate::controllers::ApiError;
use crate::controllers::Created;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope, Verified};
use crate::controllers::openapi::Routes;
use axum::Json;
use axum::extract::{Path, Query, State};
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
) -> Result<Created<model::ForkRecord>, ApiError> {
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
    Ok(Created(fork))
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
) -> Result<axum::response::NoContent, ApiError> {
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
    Ok(axum::response::NoContent)
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("enterprise")
        .prefix("api/schema-forks")
        .add(
            "",
            aide::axum::routing::get_with(list, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_list"]),
                ))
            }),
        )
        .add(
            "",
            aide::axum::routing::post_with(create, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_create"]),
                ))
            }),
        )
        .add(
            "/{fork_id}",
            aide::axum::routing::get_with(get, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_get"]),
                ))
            }),
        )
        .add(
            "/{fork_id}",
            aide::axum::routing::put_with(update, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_update"]),
                ))
            }),
        )
        .add(
            "/{fork_id}",
            aide::axum::routing::delete_with(delete, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["schema_fork_delete"]),
                ))
            }),
        )
}
