use super::boot_request;
use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel};
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use super::fixtures::{self, TenantArgs, TestTenant};

#[test]
fn concurrent_import_workers_keep_each_apps_database() {
    let run = || {
        super::fill_defaults::boot_request::<App, _, _>(|request, ctx| async move {
            let setup = TestTenant::create(
                &request,
                &ctx,
                TenantArgs {
                    key_scope: ApiKeyScope::Schema,
                    ..Default::default()
                },
            )
            .await;
            let accepted = request
                .post("/api/import.jsonl")
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
                .text("")
                .await;
            assert_eq!(accepted.status_code(), StatusCode::ACCEPTED);
            let id = accepted.json::<serde_json::Value>()["job_id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            super::fill_defaults::finish_via_api(
                &request,
                &ctx,
                id,
                &setup.api_key,
                &format!("/api/import-jobs/{id}"),
            )
            .await;
            let status = request
                .get(&format!("/api/import-jobs/{id}"))
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
                .await;
            assert_eq!(status.status_code(), StatusCode::OK);
            assert_eq!(status.json::<serde_json::Value>()["status"], "completed");
        })
    };
    // Independent runtimes must not nest the process-environment read guard on one thread.
    let worker = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run());
    };
    std::thread::scope(|scope| {
        let first = scope.spawn(worker);
        let second = scope.spawn(worker);
        first.join().unwrap();
        second.join().unwrap();
    });
}

#[tokio::test]
async fn import_status_is_scoped_to_workspace_permission_and_job_type() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            owner_id: user_id,
            api_key: key,
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let read_key = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            workspace_id,
            ApiKeyScope::Read,
            Some(user_id),
            false,
        )
        .await
        .unwrap();
        let txn = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
        let other_workspace = yorishiro::models::tenancy::create_workspace(
            &txn,
            tenant_id,
            &format!("other-{}", uuid::Uuid::new_v4()),
            None,
            None,
        )
        .await
        .expect("create another workspace");
        txn.commit().await.unwrap();
        let other_key = fixtures::issue_api_key(
            &ctx,
            other_workspace.id,
            user_id,
            ApiKeyScope::Migration,
            false,
        )
        .await;
        let accepted = request
            .post("/api/import.jsonl")
            .add_header("Authorization", format!("Bearer {key}"))
            .text("")
            .await;
        assert_eq!(accepted.status_code(), StatusCode::ACCEPTED);
        let id = accepted.json::<serde_json::Value>()["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let path = format!("/api/import-jobs/{id}");
        let own = request
            .get(&path)
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(own.status_code(), StatusCode::OK);
        let foreign = request
            .get(&path)
            .add_header("Authorization", format!("Bearer {other_key}"))
            .await;
        assert_eq!(foreign.status_code(), StatusCode::NOT_FOUND);
        let denied = request
            .get(&path)
            .add_header("Authorization", format!("Bearer {}", read_key.plaintext))
            .await;
        assert_eq!(denied.status_code(), StatusCode::FORBIDDEN);
        let wrong_type = request
            .get(&format!("/api/migration-jobs/{id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(wrong_type.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

/// A schema line is only present in the import file when the exporting workspace's own schema is being restored.
/// An entity referencing a schema that already exists in the destination workspace (created directly, not via this import) has to fall through to a lookup by id instead, and that lookup is memoized (`import::import_jsonl`'s `schema_name_by_id` cache) since a batch of entities sharing one pre-existing schema is the common case.
///
/// Two entities against the same pre-existing schema exercises both the cache miss (first entity)
/// and the cache hit (second entity), and asserts both actually land with the right entity_type
/// rather than just checking the import didn't error.
#[tokio::test]
async fn import_resolves_a_pre_existing_schema_for_every_entity_line() {
    super::fill_defaults::boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant { api_key: key, .. } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let auth = format!("Bearer {key}");

        let schema_response = request
            .post("/api/schemas")
            .add_header("Authorization", auth.clone())
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(schema_response.status_code(), StatusCode::CREATED);
        let schema_id = schema_response.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .expect("schema id")
            .to_string();

        let body = format!(
            "{}\n{}\n",
            serde_json::json!({
                "kind": "entity",
                "record": {
                    "id": uuid::Uuid::new_v4(),
                    "workspace_id": uuid::Uuid::new_v4(),
                    "schema_id": schema_id,
                    "schema_version": 1,
                    "entity_type": "project",
                    "data": { "title": "first", "status": "active" },
                    "created_at": chrono::Utc::now(),
                    "updated_at": chrono::Utc::now(),
                    "created_by": null,
                    "updated_by": null,
                }
            }),
            serde_json::json!({
                "kind": "entity",
                "record": {
                    "id": uuid::Uuid::new_v4(),
                    "workspace_id": uuid::Uuid::new_v4(),
                    "schema_id": schema_id,
                    "schema_version": 1,
                    "entity_type": "project",
                    "data": { "title": "second", "status": "active" },
                    "created_at": chrono::Utc::now(),
                    "updated_at": chrono::Utc::now(),
                    "created_by": null,
                    "updated_by": null,
                }
            }),
        );

        let import_response = request
            .post("/api/import.jsonl")
            .add_header("Authorization", auth.clone())
            .text(body)
            .await;
        assert_eq!(
            import_response.status_code(),
            StatusCode::ACCEPTED,
            "response: {:?}",
            import_response.text()
        );
        let accepted: serde_json::Value = import_response.json();
        let id = accepted["job_id"].as_str().unwrap().parse().unwrap();
        let before = request
            .get("/api/entities?entity_type=project")
            .add_header("Authorization", auth.clone())
            .await;
        assert!(before.json::<Vec<serde_json::Value>>().is_empty());
        super::fill_defaults::finish_via_api(
            &request,
            &ctx,
            id,
            &key,
            &format!("/api/import-jobs/{id}"),
        )
        .await;
        let status = request
            .get(&format!("/api/import-jobs/{id}"))
            .add_header("Authorization", auth.clone())
            .await;
        assert_eq!(status.status_code(), StatusCode::OK, "{}", status.text());
        let completed: serde_json::Value = status.json();
        assert_eq!(completed["status"], "completed");
        let result = &completed["result"];
        assert_eq!(result["entities"], 2, "result: {result:?}");
        assert_eq!(result["schemas"], 0, "result: {result:?}");

        // Simulate a crash after effects commit but before lifecycle completion is persisted.
        let mut lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        lifecycle.status = Set("queued".to_owned());
        lifecycle.completed_at = Set(None);
        lifecycle.update(&ctx.db).await.unwrap();
        let dispatch =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
        yorishiro::models::queue_job_dispatch_outbox::record_lifecycle_payload(
            &ctx.db,
            id,
            dispatch.payload,
            dispatch.worker_name,
            dispatch.queue_name,
            dispatch
                .tags
                .map(serde_json::from_value)
                .transpose()
                .unwrap(),
        )
        .await
        .unwrap();
        super::fill_defaults::finish_via_api(
            &request,
            &ctx,
            id,
            &key,
            &format!("/api/import-jobs/{id}"),
        )
        .await;

        let list_response = request
            .get("/api/entities?entity_type=project")
            .add_header("Authorization", auth)
            .await;
        assert_eq!(list_response.status_code(), StatusCode::OK);
        let items: Vec<serde_json::Value> = list_response.json();
        assert_eq!(items.len(), 2, "items: {items:?}");
        let titles: std::collections::BTreeSet<_> = items
            .iter()
            .map(|item| item["data"]["title"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            std::collections::BTreeSet::from(["first", "second"]),
            "both entities imported against the cached schema lookup, not just the first"
        );
    })
    .await;
}
