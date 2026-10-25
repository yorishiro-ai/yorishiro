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
            .json(&serde_json::json!({ "worker_class": "shared" }))
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
        assert_eq!(body["worker_class"], "shared");

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

/// An undeclared destination is refused without replacing the existing assignment.
#[tokio::test]
async fn an_unconfigured_target_preserves_the_old_assignment() {
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
            .json(&serde_json::json!({ "worker_class": "shared" }))
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
            StatusCode::UNPROCESSABLE_ENTITY,
            "response: {:?}",
            second.text()
        );

        let get = request
            .get("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        let body: serde_json::Value = get.json();
        assert_eq!(
            body["worker_class"], "shared",
            "a refused assignment must leave the previous destination intact"
        );

        let private = request
            .put("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({ "worker_class": "tenant_private" }))
            .await;
        assert_eq!(private.status_code(), StatusCode::UNPROCESSABLE_ENTITY);

        let mut configured = ctx.clone();
        let mut settings = configured.config.settings::<serde_json::Value>().unwrap();
        settings["worker_targets"] = serde_json::json!([
            "worker-target:shared",
            "worker-target:official",
            format!("worker-target:tenant:{}", setup.tenant_id)
        ]);
        configured.config.settings = Some(settings);
        let mut wrong_tenant = configured.clone();
        let mut settings = wrong_tenant.config.settings::<serde_json::Value>().unwrap();
        settings["worker_targets"] =
            serde_json::json!([format!("worker-target:tenant:{}", uuid::Uuid::now_v7())]);
        wrong_tenant.config.settings = Some(settings);
        assert!(matches!(
            yorishiro::edition::ee::models::workspace_worker_classes::set(
                &wrong_tenant,
                setup.workspace_id,
                yorishiro::workers::embedding_sync::WorkerClass::TenantPrivate,
            )
            .await,
            Err(yorishiro::YorishiroError::ValidationFailed { .. })
        ));
        for class in [
            yorishiro::workers::embedding_sync::WorkerClass::Official,
            yorishiro::workers::embedding_sync::WorkerClass::TenantPrivate,
        ] {
            yorishiro::edition::ee::models::workspace_worker_classes::set(
                &configured,
                setup.workspace_id,
                class,
            )
            .await
            .unwrap();
            let get = request
                .get("/api/workspace/worker-class")
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
                .await;
            let body: serde_json::Value = get.json();
            assert_eq!(body["worker_class"], class.as_db_str());
        }
    })
    .await;
}
