use super::*;

/// The REST search endpoint deserializes its parameters from the query string.
/// Only `query_text` is required; the rest narrow an already-valid search, so a caller sending just the text must succeed.
#[test]
fn only_the_query_text_is_required() {
    let params: SearchEntitiesParams =
        serde_json::from_value(serde_json::json!({ "query_text": "how do I deploy" })).unwrap();

    assert_eq!(params.query_text, "how do I deploy");
    assert!(params.entity_type.is_none());
    assert!(params.filter.is_none());
}

/// A search with no text is meaningless (it would embed to noise), so the parameter is required rather than defaulted to an empty string.
#[test]
fn a_search_without_text_is_rejected() {
    assert!(
        serde_json::from_value::<SearchEntitiesParams>(
            serde_json::json!({ "entity_type": "task" })
        )
        .is_err()
    );
}

/// The filter arrives as a raw string on the query string and is parsed separately by `parse_filter_param`; at this layer it stays a string so the endpoint can report a precise error for malformed JSON.
#[test]
fn the_filter_is_carried_as_a_raw_string_for_later_parsing() {
    let params: SearchEntitiesParams = serde_json::from_value(serde_json::json!({
        "query_text": "anything",
        "filter": "{\"status\":\"active\"}"
    }))
    .unwrap();

    assert!(params.filter.is_some());
}

use std::sync::Arc;

use crate::test_support::*;
use crate::{AppState, build_app};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

/// Issues a key for a freshly seeded workspace and returns an app wired to `provider`.
async fn app_with_key(
    pool: &PgPool,
    scope: ApiKeyScope,
    provider: Arc<dyn yorishiro_core::services::embedding::EmbeddingProvider>,
) -> (axum::Router, String) {
    let (tenant_id, workspace_id) = seed_workspace(pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let created = create_api_key(&mut *conn, workspace_id, scope, None)
        .await
        .unwrap();
    drop(conn);

    let app = build_app(
        AppState::new(db, pool.clone(), provider),
        no_static_fallback(),
    );
    (app, created.plaintext)
}

/// Creates a schema with one embeddable field and one entity carrying `title`, then embeds it synchronously.
/// `POST /api/entities` would embed in a background task, which this test would have to poll for; going through the same repository and service calls that task uses gets the row and its vector committed before the search runs.
async fn seed_embedded_entity(
    conn: &mut sqlx::PgConnection,
    tenant_id: Uuid,
    workspace_id: Uuid,
    title: &str,
) {
    let definition = serde_json::from_value(serde_json::json!({
        "name": "notes",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "x-embed": true } } }
        }
    }))
    .unwrap();
    yorishiro_core::models::schemas::create_schema(conn, tenant_id, workspace_id, definition)
        .await
        .unwrap();

    let record = yorishiro_core::models::entities::create(
        conn,
        workspace_id,
        yorishiro_core::models::entities::CreateEntityInput {
            schema_name: "notes".into(),
            entity_type: "note".into(),
            data: serde_json::json!({ "title": title }),
        },
        None,
    )
    .await
    .unwrap();

    yorishiro_core::services::embedding::sync::sync_embedding_for_record(
        conn,
        workspace_id,
        &record,
        &FixedEmbeddingProvider,
    )
    .await
    .unwrap();
}

async fn get(app: axum::Router, uri: &str, key: &str) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {key}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

/// Search reads entity data, so an unauthenticated caller must not reach the embedding provider:
/// `UnreachableEmbeddingProvider` errors if it is called, which would surface as a 500 rather than the 401 this asserts.
#[sqlx::test(migrations = "../../migrations")]
async fn search_requires_authentication(pool: PgPool) {
    let app = build_app(test_state(pool), no_static_fallback());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/search?query_text=anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// `query_text` is the one required parameter.
/// Axum rejects the request at extraction, before the handler runs.
/// This also pins that no embedding call is made for a request that cannot produce a meaningful search.
#[sqlx::test(migrations = "../../migrations")]
async fn a_search_without_query_text_is_rejected_before_embedding(pool: PgPool) {
    let (app, key) = app_with_key(
        &pool,
        ApiKeyScope::Read,
        Arc::new(UnreachableEmbeddingProvider),
    )
    .await;

    let response = get(app, "/api/search", &key).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// A malformed `filter` has to come back as a client error naming the problem.
/// Returning a 500 would tell the caller to retry a request that can never succeed.
#[sqlx::test(migrations = "../../migrations")]
async fn a_malformed_filter_is_a_client_error(pool: PgPool) {
    let (app, key) = app_with_key(
        &pool,
        ApiKeyScope::Read,
        Arc::new(UnreachableEmbeddingProvider),
    )
    .await;

    let response = get(app, "/api/search?query_text=x&filter=not-json", &key).await;

    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a filter that is not JSON is the caller's mistake, not the server's"
    );
}

/// Search hands the caller's `workspace_id` to `search_by_vector` separately from acquiring the connection.
/// If it ever passed the wrong one (or none), one tenant's search would return another's entities, and `FixedEmbeddingProvider` makes every entity a distance-0 match, so nothing about the vector distance would hide the leak.
///
/// The entity is written directly rather than through `POST /api/entities` because that path embeds in a background task; this test is about which workspace the *query* reads, and `tests/http/controllers/entities.rs` already covers the write-then-search round trip.
#[sqlx::test(migrations = "../../migrations")]
async fn a_search_never_returns_another_workspaces_entities(pool: PgPool) {
    let db = TenantDb::new(pool.clone());

    // Two separate tenants, each with an embedded entity carrying identical text.
    let mut keys = Vec::new();
    for owner in ["first", "second"] {
        let (tenant_id, workspace_id) = seed_workspace(&pool).await;
        let mut conn = db
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();
        seed_embedded_entity(&mut conn, tenant_id, workspace_id, owner).await;
        let created = create_api_key(&mut *conn, workspace_id, ApiKeyScope::Read, None)
            .await
            .unwrap();
        drop(conn);
        keys.push((owner, created.plaintext));
    }

    for (owner, key) in &keys {
        let app = build_app(
            AppState::new(db.clone(), pool.clone(), Arc::new(FixedEmbeddingProvider)),
            no_static_fallback(),
        );
        let response = get(app, "/api/search?query_text=anything&limit=50", key).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let hits: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let titles: Vec<&str> = hits
            .as_array()
            .expect("hits must be a JSON array")
            .iter()
            .filter_map(|h| h["entity"]["data"]["title"].as_str())
            .collect();

        assert_eq!(
            titles,
            vec![*owner],
            "the {owner} workspace's search must return exactly its own entity"
        );
    }
}
