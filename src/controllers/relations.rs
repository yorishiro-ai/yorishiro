//! REST routes for entity relations.

use crate::controllers::Created;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{AuthContext, Authorized, ReadScope, Verified, WriteScope};
use crate::controllers::openapi::Routes;
use crate::dtos::relations::{
    CreateRelationRequest, ListRelationsParams, SetRelationStatusRequest,
};
use crate::models::entity_relations::{self, RelationRecord};

pub(crate) async fn create_relation(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Json(body): Json<CreateRelationRequest>,
) -> Result<Created<RelationRecord>, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        crate::models::entity_entities::workspace_for_actor(&ctx.db, &verified.ctx, body.source_id)
            .await?,
    );
    let mut authorized = Authorized::<WriteScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::RelationCreate)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let input = body.into();
    let record = entity_relations::create(authorized.txn(), workspace_id, input).await?;
    authorized.commit().await?;
    Ok(Created(record))
}

pub(crate) async fn get_relation(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<Json<RelationRecord>, ApiError> {
    let mut authorized = Authorized::<ReadScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Relation(id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::RelationGet)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let record = entity_relations::get(authorized.txn(), workspace_id, id).await?;
    Ok(Json(record))
}

pub(crate) async fn delete_relation(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    let mut authorized = Authorized::<WriteScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Relation(id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::RelationDelete)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    entity_relations::delete(authorized.txn(), workspace_id, id).await?;
    authorized.commit().await?;
    Ok(axum::response::NoContent)
}

pub(crate) async fn list_relations(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Query(params): Query<ListRelationsParams>,
) -> Result<Json<Vec<RelationRecord>>, ApiError> {
    let query = params.try_into()?;

    let records = entity_relations::list_for_actor(&ctx, &actor, query).await?;
    Ok(Json(records))
}

pub(crate) async fn set_relation_status(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Json(body): Json<SetRelationStatusRequest>,
) -> Result<Json<RelationRecord>, ApiError> {
    let mut authorized = Authorized::<WriteScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Relation(id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::RelationUpdate)
        .await?;
    let workspace_id = authorized.ctx.require_workspace()?;
    let record = entity_relations::set_status(
        authorized.txn(),
        workspace_id,
        entity_relations::SetRelationStatusInput {
            id,
            status: body.status,
        },
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(record))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/relations")
        .add(
            "/",
            aide::axum::routing::post_with(create_relation, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["relation_create"]),
                ))
            }),
        )
        .add(
            "/",
            aide::axum::routing::get_with(list_relations, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["relation_list"]),
                ))
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::get_with(get_relation, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["relation_get"]),
                ))
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::delete_with(delete_relation, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["relation_delete"]),
                ))
            }),
        )
        .add(
            "/{id}/status",
            aide::axum::routing::put_with(set_relation_status, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["relation_update"]),
                ))
            }),
        )
}
