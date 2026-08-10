use std::sync::Arc;

use crate::test_support::*;
use crate::{AppState, build_app};
use axum::http::StatusCode;
use sqlx::PgPool;
use yorishiro_core::db::TenantDb;
use yorishiro_core::services::auth::{ApiKeyScope, create_api_key};

fn task_schema() -> serde_json::Value {
    serde_json::json!({
        "name": "task-management",
        "entity_types": {
            "task": { "fields": { "title": { "type": "string", "required": true } } }
        }
    })
}

fn task_schema_v2() -> serde_json::Value {
    serde_json::json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": {
                    "title": { "type": "string", "required": true },
                    "priority": { "type": "string", "required": false }
                }
            }
        }
    })
}

/// Walks the whole lifecycle over HTTP: fork, edit, notice the tenant moved on, refuse to
/// discard the edit, then follow with force.
#[sqlx::test(migrations = "../../migrations")]
async fn rest_workspace_schema_fork_lifecycle(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let schema_key = create_api_key(
        &mut conn,
        tenant_id,
        Some(workspace_id),
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
    let auth = format!("Bearer {}", schema_key.plaintext);

    // The tenant's schema, which the workspace uses directly at first.
    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&auth),
        Some(task_schema()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(&app, "GET", "/api/workspace-schema", Some(&auth), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = rest_json_body(response).await;
    assert!(body["fork"].is_null(), "no fork until one is asked for");

    // Fork it.
    let response = rest_request(
        &app,
        "POST",
        "/api/workspace-schema",
        Some(&auth),
        Some(serde_json::json!({ "schema_name": "task-management" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let fork = rest_json_body(response).await;
    assert_eq!(fork["source_version"], 1);
    assert_eq!(fork["customized"], false);

    // Edit the fork.
    let response = rest_request(
        &app,
        "PUT",
        "/api/workspace-schema",
        Some(&auth),
        Some(task_schema_v2()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(rest_json_body(response).await["customized"], true);

    // The tenant edits its own schema, so the fork is now behind.
    let response = rest_request(
        &app,
        "POST",
        "/api/schemas",
        Some(&auth),
        Some(task_schema_v2()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = rest_request(&app, "GET", "/api/workspace-schema", Some(&auth), None).await;
    let body = rest_json_body(response).await;
    assert_eq!(body["upstream_version"], 2);

    // Following would discard the workspace's own edit, so it is refused.
    let response = rest_request(
        &app,
        "POST",
        "/api/workspace-schema/follow",
        Some(&auth),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // Saying so explicitly goes through.
    let response = rest_request(
        &app,
        "POST",
        "/api/workspace-schema/follow",
        Some(&auth),
        Some(serde_json::json!({ "force": true })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let followed = rest_json_body(response).await;
    assert_eq!(followed["source_version"], 2);
    assert_eq!(followed["customized"], false);

    // Dropping the fork returns the workspace to its tenant's schema.
    let response = rest_request(&app, "DELETE", "/api/workspace-schema", Some(&auth), None).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = rest_request(&app, "GET", "/api/workspace-schema", Some(&auth), None).await;
    assert!(rest_json_body(response).await["fork"].is_null());
}

/// Editing or following without a fork is a 404 rather than an implicit fork -- creating one as
/// a side effect of an edit would hide which workspaces have diverged.
#[sqlx::test(migrations = "../../migrations")]
async fn rest_workspace_schema_requires_an_existing_fork(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let schema_key = create_api_key(
        &mut conn,
        tenant_id,
        Some(workspace_id),
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
    let auth = format!("Bearer {}", schema_key.plaintext);

    for (method, path, body) in [
        ("PUT", "/api/workspace-schema", Some(task_schema())),
        (
            "POST",
            "/api/workspace-schema/follow",
            Some(serde_json::json!({})),
        ),
        ("DELETE", "/api/workspace-schema", None),
    ] {
        let response = rest_request(&app, method, path, Some(&auth), body).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{method} {path} without a fork"
        );
    }
}

/// Forking changes what the workspace validates against, so it needs schema scope like every
/// other schema-shaped operation.
#[sqlx::test(migrations = "../../migrations")]
async fn rest_workspace_schema_fork_requires_schema_scope(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let write_key = create_api_key(
        &mut conn,
        tenant_id,
        Some(workspace_id),
        ApiKeyScope::Write,
        None,
    )
    .await
    .unwrap();
    drop(conn);

    let app = build_app(
        AppState::new(db, pool.clone(), Arc::new(UnreachableEmbeddingProvider)),
        None,
    );
    let auth = format!("Bearer {}", write_key.plaintext);

    let response = rest_request(
        &app,
        "POST",
        "/api/workspace-schema",
        Some(&auth),
        Some(serde_json::json!({ "schema_name": "task-management" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
