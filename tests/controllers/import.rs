use crate::requests::boot_request;
use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel, TransactionTrait};
use yorishiro::App;

use crate::requests::fixtures::{self, TenantArgs, TestTenant};

#[test]
fn concurrent_import_workers_keep_each_apps_database() {
    let run = || {
        boot_request::<App, _, _>(|request, ctx| async move {
            let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
            let accepted = request
                .post("/api/import-jobs")
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
                .text("")
                .await;
            assert_eq!(accepted.status_code(), StatusCode::ACCEPTED);
            let id = accepted.json::<serde_json::Value>()["import_job_id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            crate::requests::jobs::finish_via_api(
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
        let administrator = fixtures::create_iam_reader(&ctx, tenant_id).await;
        let read_key = fixtures::create_key(&ctx.db, workspace_id, Some(administrator))
            .await
            .unwrap();
        let txn = TransactionTrait::begin(&ctx.db).await.unwrap();
        let other_workspace = yorishiro::models::tenancy::create_workspace(
            &txn,
            tenant_id,
            &format!("other-{}", uuid::Uuid::new_v4()),
            None,
        )
        .await
        .expect("create another workspace");
        txn.commit().await.unwrap();
        let other_key = fixtures::issue_access_key(&ctx, other_workspace.id, user_id).await;
        let accepted = request
            .post("/api/import-jobs")
            .add_header("Authorization", format!("Bearer {key}"))
            .add_header("x-workspace-id", workspace_id.to_string())
            .text("")
            .await;
        assert_eq!(accepted.status_code(), StatusCode::ACCEPTED);
        let id = accepted.json::<serde_json::Value>()["import_job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let path = format!("/api/import-jobs/{id}");
        let own = request
            .get(&path)
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(own.status_code(), StatusCode::OK);
        let root = fixtures::issue_tenant_key(&ctx, tenant_id, user_id).await;
        request
            .get(&path)
            .add_header("Authorization", format!("Bearer {root}"))
            .await
            .assert_status_ok();
        let txn = ctx.db.begin().await.unwrap();
        let ungranted = yorishiro::models::tenancy::register_member(
            &txn,
            tenant_id,
            &fixtures::unique_email("import-status"),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let ungranted = fixtures::issue_tenant_key(&ctx, tenant_id, ungranted.id).await;
        request
            .get(&path)
            .add_header("Authorization", format!("Bearer {ungranted}"))
            .await
            .assert_status(StatusCode::NOT_FOUND);
        request
            .get(&path)
            .add_header("Authorization", format!("Bearer {ungranted}"))
            .add_header("x-workspace-id", workspace_id.to_string())
            .await
            .assert_status(StatusCode::FORBIDDEN);
        let foreign = request
            .get(&path)
            .add_header("Authorization", format!("Bearer {other_key}"))
            .add_header("x-workspace-id", other_workspace.id.to_string())
            .await;
        assert_eq!(foreign.status_code(), StatusCode::NOT_FOUND);
        // The built-in reader policy includes `import_job_get`.
        request
            .get(&path)
            .add_header("Authorization", format!("Bearer {}", read_key.plaintext))
            .await
            .assert_status_ok();
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
/// and the cache hit (second entity), and asserts both actually land with the right `entity_type`
/// rather than just checking the import didn't error.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one import checks cached schema resolution and replays committed effects to verify crash recovery does not duplicate entities"
)]
async fn import_resolves_a_pre_existing_schema_for_every_entity_line() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant { api_key: key, .. } =
            TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let auth = format!("Bearer {key}");

        let schema_response = request
            .post("/api/schemas")
            .add_header("Authorization", auth.clone())
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(schema_response.status_code(), StatusCode::CREATED);
        let schema_id = schema_response.json::<serde_json::Value>()["schema"]["schema_id"]
            .as_str()
            .expect("schema id")
            .to_string();

        let [first, second] = ["first", "second"].map(|title| {
            serde_json::json!({
                        "kind": "entity",
                        "record": {
                            "id": uuid::Uuid::new_v4(),
                            "workspace_id": uuid::Uuid::new_v4(),
                            "schema_id": schema_id,
                            "schema_version": 1,
                            "entity_type": "project",
                            "data": { "title": title, "status": "active" },
                            "created_at": chrono::Utc::now(),
                            "updated_at": chrono::Utc::now(),
                            "created_by": null,
                            "updated_by": null,
                        }
            })
        });
        let body = format!("{first}\n{second}\n");

        let import_response = request
            .post("/api/import-jobs")
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
        let id = accepted["import_job_id"].as_str().unwrap().parse().unwrap();
        let before = request
            .get("/api/entities?entity_type=project")
            .add_header("Authorization", auth.clone())
            .await;
        assert_eq!(
            before.json::<Vec<serde_json::Value>>(),
            [] as [serde_json::Value; 0]
        );
        crate::requests::jobs::finish_via_api(
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
        let replay = ctx
            .db
            .begin_with_options(sea_orm::TransactionOptions {
                sqlite_transaction_mode: Some(sea_orm::SqliteTransactionMode::Immediate),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(id)
            .one(&replay)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        lifecycle.status = Set("queued".to_owned());
        lifecycle.completed_at = Set(None);
        lifecycle.update(&replay).await.unwrap();
        let dispatch =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
                .one(&replay)
                .await
                .unwrap()
                .unwrap();
        yorishiro::models::queue_job_dispatch_outbox::record_lifecycle_payload(
            &replay,
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
        replay.commit().await.unwrap();
        crate::requests::jobs::finish_via_api(
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

#[tokio::test]
async fn embedding_jobs_of_an_import_keep_the_request_that_started_it() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::queue_job_lifecycles as lifecycles;
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let auth = format!("Bearer {}", tenant.api_key);
        let schema = request
            .post("/api/schemas")
            .add_header("Authorization", auth.clone())
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        let schema_id = schema.json::<serde_json::Value>()["schema"]["schema_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let line = serde_json::json!({"kind": "entity", "record": {
            "id": uuid::Uuid::new_v4(), "workspace_id": uuid::Uuid::new_v4(),
            "schema_id": schema_id, "schema_version": 1, "entity_type": "project",
            "data": { "title": "child", "status": "active" },
            "created_at": chrono::Utc::now(), "updated_at": chrono::Utc::now(),
            "created_by": null, "updated_by": null,
        }});
        let accepted = request
            .post("/api/import-jobs")
            .add_header("Authorization", auth.clone())
            .add_header("x-request-id", "import-parent")
            .text(format!("{line}\n"))
            .await;
        accepted.assert_status(StatusCode::ACCEPTED);
        let id = accepted.json::<serde_json::Value>()["import_job_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        crate::requests::jobs::finish_via_api(
            &request,
            &ctx,
            id,
            &tenant.api_key,
            &format!("/api/import-jobs/{id}"),
        )
        .await;
        let children = lifecycles::Entity::find()
            .filter(lifecycles::Column::WorkspaceId.eq(tenant.workspace_id))
            .filter(lifecycles::Column::JobName.eq("embedding_sync"))
            .all(&ctx.db)
            .await
            .unwrap();
        assert!(!children.is_empty(), "the import enqueued an embedding job");
        assert!(
            children
                .iter()
                .all(|job| job.request_id.as_deref() == Some("import-parent")),
            "{children:?}"
        );
    })
    .await;
}
