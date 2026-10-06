use axum::Json;
use axum::extract::{Query, State};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use sea_orm::TransactionTrait;

use crate::controllers::ApiError;
use crate::controllers::extractors::{ReadScope, Verified, db_handle, search_token_limiter};
use crate::controllers::middleware::rate_limit::charge_search_tokens;
use crate::db::AppContextBackend;
use crate::dtos::search::SearchEntitiesParams;
use crate::error::ResultExt;
use crate::models::search::{self, SearchHit};

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/search", params(("query_text" = String, Query, description = "Text to embed and search for"), ("entity_type" = Option<String>, Query), ("filter" = Option<String>, Query, description = "JSON-encoded containment filter"), ("limit" = Option<i64>, Query)), responses((status = 200, body = [crate::models::search::SearchHit]), (status = 401, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody), (status = 503, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "community"))]
pub(crate) async fn search_entities(
    State(ctx): State<AppContext>,
    // `Verified`, not `Authorized`: no connection is acquired here until after the slow embedding call below.
    verified: Verified<ReadScope>,
    Query(params): Query<SearchEntitiesParams>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let default = search::SearchQuery::default();
    let query = search::SearchQuery {
        entity_type: params.entity_type,
        filter: crate::dtos::common::parse_filter_param(params.filter)?,
        limit: params.limit.unwrap_or(default.limit),
    };

    let workspace_id = verified.ctx.workspace_id;
    let limiter = search_token_limiter(&ctx)?;
    charge_search_tokens(&limiter, workspace_id, &params.query_text)?;

    // The server holds no model: a worker embeds the query, and no DB connection is held while waiting for it.
    let vector = crate::workers::query_embedding::embed_query(
        &ctx,
        verified.ctx.tenant_id,
        workspace_id,
        &params.query_text,
    )
    .await?;

    // The tenant-scoped role cannot read `tenant_tenants`, which the embedding chain joins, so the table is resolved on the identity pool before the transaction opens.
    let embed_table = search::resolve_query_table(&ctx.db, workspace_id, vector.len()).await?;

    // A read-only transaction: dropped without committing when this returns, a no-op since nothing was written.
    let txn = if ctx.is_sqlite() {
        ctx.db.begin().await.internal()?
    } else {
        let db = db_handle(&ctx)?;
        db.tenant
            .begin_for_workspace(verified.ctx.tenant_id, workspace_id)
            .await
            .internal()?
    };

    let hits = search::search_in_table(
        &txn,
        workspace_id,
        vector,
        &params.query_text,
        query,
        embed_table,
    )
    .await?;
    Ok(Json(hits))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![super::route_inventory::path_doc(__path_search_entities)]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/search")
        .add("/", axum::routing::get(search_entities))
}
