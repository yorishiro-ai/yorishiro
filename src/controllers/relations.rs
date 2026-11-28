//! REST routes for entity relations.

use crate::controllers::Created;
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{AuthContext, Verified};
use crate::controllers::openapi::Routes;
use crate::dtos::common::RelationIdPath;
use crate::dtos::relations::{CreateRelationRequest, ListRelationsParams, UpdateRelationRequest};
use crate::models::entity_relations::{self, RelationRecord};

pub(crate) async fn create_relation(
    State(ctx): State<AppContext>,
    verified: Verified,
    Json(body): Json<CreateRelationRequest>,
) -> Result<Created<RelationRecord>, ApiError> {
    Ok(Created(
        entity_relations::create_for_actor(&ctx, verified.ctx, body.into()).await?,
    ))
}

pub(crate) async fn get_relation(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(RelationIdPath { relation_id }): Path<RelationIdPath<Uuid>>,
) -> Result<Json<RelationRecord>, ApiError> {
    Ok(Json(
        entity_relations::get_for_actor(&ctx, verified.ctx, relation_id).await?,
    ))
}

pub(crate) async fn delete_relation(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(RelationIdPath { relation_id }): Path<RelationIdPath<Uuid>>,
) -> Result<axum::response::NoContent, ApiError> {
    entity_relations::delete_for_actor(&ctx, verified.ctx, relation_id).await?;
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

pub(crate) async fn update_relation(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(RelationIdPath { relation_id }): Path<RelationIdPath<Uuid>>,
    Json(body): Json<UpdateRelationRequest>,
) -> Result<Json<RelationRecord>, ApiError> {
    Ok(Json(
        entity_relations::set_status_for_actor(
            &ctx,
            verified.ctx,
            entity_relations::SetRelationStatusInput {
                id: relation_id,
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
            aide::axum::routing::post_with(create_relation, |op| op.id("CreateRelation")),
        )
        .add(
            "/",
            aide::axum::routing::get_with(list_relations, |op| op.id("ListRelations")),
        )
        .add(
            "/{relation_id}",
            aide::axum::routing::get_with(get_relation, |op| op.id("GetRelation")),
        )
        .add(
            "/{relation_id}",
            aide::axum::routing::delete_with(delete_relation, |op| op.id("DeleteRelation")),
        )
        .add(
            "/{relation_id}",
            aide::axum::routing::patch_with(update_relation, |op| op.id("UpdateRelation")),
        )
}
