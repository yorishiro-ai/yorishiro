use std::time::Duration;

use loco_rs::{app::AppContext, bgworker::BackgroundWorker};
use sea_orm::EntityTrait;
use uuid::Uuid;
use yorishiro::workers::{fill_defaults::FillDefaultsWorker, target::WorkerTarget};

pub(crate) async fn finish(ctx: &AppContext, id: Uuid) {
    let queue = super::query_worker::shared_target_queue(ctx).await;
    queue
        .register(FillDefaultsWorker::build(ctx))
        .await
        .expect("register migration worker");
    let mut publisher = ctx.clone();
    publisher.queue_provider = Some(queue.clone());
    yorishiro::workers::dispatch::recover_pending(&publisher).await;
    let running = queue.clone();
    let worker = tokio::spawn(async move { running.run(vec![WorkerTarget::Shared.tag()]).await });
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let row = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_ne!(row.status, "failed", "worker failed: {:?}", row.error);
            if row.status == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    queue.shutdown().unwrap();
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    result.expect("migration must complete on the configured queue provider");
}

#[tokio::test]
#[serial_test::serial(process_environment, queue_backend)]
async fn rest_fill_defaults_waits_for_a_loco_worker() {
    use axum::http::StatusCode;
    use serde_json::json;
    use yorishiro::models::{api_keys::ApiKeyScope, entity_entities, schema_schemas};

    super::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let (tenant, workspace, _, key) = super::fixtures::create_tenant_workspace_owner(
            &ctx, super::fixtures::TenantArgs { key_scope: ApiKeyScope::Migration, ..Default::default() },
        ).await;
        let mut entity_id = None;
        for (version, fields) in [json!({"title": {"type": "string", "required": true}}),
            json!({"title": {"type": "string", "required": true}, "rank": {"type": "integer", "required": true, "default": 7}})]
            .into_iter().enumerate()
        {
            schema_schemas::create_schema(&ctx.db, tenant, workspace,
                serde_json::from_value(json!({"name": "note", "entity_types": {"note": {"fields": fields}}})).unwrap(),
                None, None).await.unwrap();
            if version == 0 {
                entity_id = Some(entity_entities::create(&ctx.db, workspace, entity_entities::CreateEntityInput {
                    schema_name: "note".into(), entity_type: "note".into(), data: json!({"title": "before"}),
                }, None).await.unwrap().id);
            }
        }
        let response = request.post("/api/migration-jobs/fill-defaults")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&json!({"schema_name": "note"})).await;
        assert_eq!(response.status_code(), StatusCode::ACCEPTED, "{}", response.text());
        let id = response.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        let entity_id = entity_id.unwrap();
        let before = entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap();
        assert_eq!(before.data, json!({"title": "before"}));
        finish(&ctx, id).await;
        let after = entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap();
        assert_eq!(after.data, json!({"title": "before", "rank": 7}));
    }).await;
}
