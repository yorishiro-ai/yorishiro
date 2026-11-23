//! REST routes for entity relations.

use crate::controllers::Created;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{AuthContext, ReadScope, Verified, WriteScope};
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
    Ok(Created(
        entity_relations::create_for_actor(&ctx, verified.ctx, body.into()).await?,
    ))
}

pub(crate) async fn get_relation(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<Json<RelationRecord>, ApiError> {
    Ok(Json(
        entity_relations::get_for_actor(&ctx, verified.ctx, id).await?,
    ))
}

pub(crate) async fn delete_relation(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    entity_relations::delete_for_actor(&ctx, verified.ctx, id).await?;
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
    Ok(Json(
        entity_relations::set_status_for_actor(
            &ctx,
            verified.ctx,
            entity_relations::SetRelationStatusInput {
                id,
                status: body.status,
            },
        )
        .await?,
    ))
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
