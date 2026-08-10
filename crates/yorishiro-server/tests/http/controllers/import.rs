use std::sync::Arc;

use crate::test_support::*;
use crate::{AppState, build_app};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

async fn ndjson_request(
    app: &axum::Router,
    uri: &str,
    auth_header: Option<&str>,
    body: &str,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-ndjson");
    if let Some(auth_header) = auth_header {
        builder = builder.header("authorization", auth_header);
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn rest_import_jsonl_round_trips_through_export(pool: PgPool) {
    let (tenant_id_tenant, tenant_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id_tenant, tenant_id)
        .await
        .unwrap();
    let schema_key = create_api_key(
        &mut conn,
        tenant_id_tenant,
        Some(tenant_id),
        ApiKeyScope::Schema,
        None,
    )
    .await
    .unwrap();
    let write_key = create_api_key(
        &mut conn,
        tenant_id_tenant,
        Some(tenant_id),
        ApiKeyScope::Write,
        None,
    )
    .await
    .unwrap();
    drop(conn);

    let app = build_app(
        AppState::new(
            db.clone(),
            pool.clone(),
            Arc::new(UnreachableEmbeddingProvider),
        ),
        None,
    );
    let schema_auth = format!("Bearer {}", schema_key.plaintext);
    let write_auth = format!("Bearer {}", write_key.plaintext);

    // Seed source data and export it.
    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&schema_auth),
        Some(serde_json::json!({
            "name": "task-management",
            "entity_types": {
                "task": { "fields": { "title": { "type": "string", "required": true } } }
            },
            "relation_types": {
                "blocks": { "source": "task", "target": "task" }
            },
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(
        &app,
        "POST",
        "/api/entities",
        Some(&write_auth),
        Some(serde_json::json!({
            "schema_name": "task-management", "entity_type": "task", "data": { "title": "a" },
        })),
    )
    .await;
    let a = rest_json_body(response).await;

    let response = rest_request(
        &app,
        "POST",
        "/api/entities",
        Some(&write_auth),
        Some(serde_json::json!({
            "schema_name": "task-management", "entity_type": "task", "data": { "title": "b" },
        })),
    )
    .await;
    let b = rest_json_body(response).await;

    let response = rest_request(
        &app,
        "POST",
        "/api/relations",
        Some(&write_auth),
        Some(serde_json::json!({
            "source_id": a["id"], "target_id": b["id"], "relation_type": "blocks",
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(&app, "GET", "/api/export.jsonl", Some(&write_auth), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let exported = std::str::from_utf8(&body).unwrap().to_string();

    // Import that export into a second, empty workspace/tenant.
    let (other_tenant_tenant, other_tenant) = seed_workspace(&pool).await;
    let mut other_conn = db
        .acquire_for_workspace(other_tenant_tenant, other_tenant)
        .await
        .unwrap();
    let other_schema_key = create_api_key(
        &mut other_conn,
        other_tenant_tenant,
        Some(other_tenant),
        ApiKeyScope::Schema,
        None,
    )
    .await
    .unwrap();
    drop(other_conn);
    let other_schema_auth = format!("Bearer {}", other_schema_key.plaintext);

    let response = ndjson_request(
        &app,
        "/api/import.jsonl",
        Some(&other_schema_auth),
        &exported,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = rest_json_body(response).await;
    assert_eq!(result["schemas"], 1);
    assert_eq!(result["entities"], 2);
    assert_eq!(result["relations"], 1);
    assert_eq!(result["errors"], serde_json::json!([]));

    // Importing again with only write scope (no schema scope) is rejected.
    let other_write_key = create_api_key(
        &mut db
            .acquire_for_workspace(other_tenant_tenant, other_tenant)
            .await
            .unwrap(),
        other_tenant_tenant,
        Some(other_tenant),
        ApiKeyScope::Write,
        None,
    )
    .await
    .unwrap();
    let other_write_auth = format!("Bearer {}", other_write_key.plaintext);
    let response = ndjson_request(
        &app,
        "/api/import.jsonl",
        Some(&other_write_auth),
        &exported,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = ndjson_request(&app, "/api/import.jsonl", None, &exported).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn rest_import_jsonl_rejects_malformed_body(pool: PgPool) {
    let (tenant_id_tenant, tenant_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id_tenant, tenant_id)
        .await
        .unwrap();
    let schema_key = create_api_key(
        &mut conn,
        tenant_id_tenant,
        Some(tenant_id),
        ApiKeyScope::Schema,
        None,
    )
    .await
    .unwrap();
    drop(conn);

    let app = build_app(
        AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
        None,
    );
    let schema_auth = format!("Bearer {}", schema_key.plaintext);

    let response = ndjson_request(
        &app,
        "/api/import.jsonl",
        Some(&schema_auth),
        "not valid jsonl",
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
