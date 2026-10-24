use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant};
use axum::http::StatusCode;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

/// Setting, reading and clearing a workspace's own worker-class assignment over REST, matching `embedding_key_set_get_and_clear_round_trip`.
#[tokio::test]
async fn worker_class_set_get_and_clear_round_trip() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

        let missing = request
            .get("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            missing.status_code(),
            StatusCode::NOT_FOUND,
            "response: {:?}",
            missing.text()
        );

        let put = request
            .put("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({ "worker_class": "tenant_private" }))
            .await;
        assert_eq!(
            put.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            put.text()
        );

        let get = request
            .get("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            get.status_code(),
            StatusCode::OK,
            "response: {:?}",
            get.text()
        );
        let body: serde_json::Value = get.json();
        assert_eq!(body["worker_class"], "tenant_private");

        let delete = request
            .delete("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            delete.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            delete.text()
        );

        let after_delete = request
            .get("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(after_delete.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

/// Re-`PUT`ting a different class replaces the assignment rather than erroring or adding a second row, matching the `ON CONFLICT` upsert `workspace_embedding_keys::set`/`workspace_llm_keys::set` both use.
#[tokio::test]
async fn setting_a_new_class_replaces_the_old_one() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

        let first = request
            .put("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({ "worker_class": "tenant_private" }))
            .await;
        assert_eq!(
            first.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            first.text()
        );

        let second = request
            .put("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({ "worker_class": "official" }))
            .await;
        assert_eq!(
            second.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            second.text()
        );

        let get = request
            .get("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        let body: serde_json::Value = get.json();
        assert_eq!(
            body["worker_class"], "official",
            "the second PUT must replace the first, not add a second row"
        );
    })
    .await;
}
