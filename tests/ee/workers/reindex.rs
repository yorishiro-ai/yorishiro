use std::sync::{Arc, Mutex};

use sea_orm::sea_query::{Alias, Expr, Func};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;
use yorishiro::models::_entities::entity_embeddings;

use yorishiro::models::_entities::workspace_workspaces;
use yorishiro::models::{entity_entities, entity_entities::CreateEntityInput, schema_schemas};
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

async fn provider(
    model: &str,
    width: usize,
    value: f32,
    calls: Arc<Mutex<usize>>,
) -> (Arc<dyn EmbeddingProvider>, crate::http_stub::HttpStub) {
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let calls = calls.clone();
            async move {
                let inputs = body["input"].as_array().expect("embedding inputs");
                *calls.lock().unwrap() += inputs.len();
                let data = inputs.iter().enumerate().map(|(index, _)| {
                    serde_json::json!({"index": index, "embedding": vec![value; width]})
                }).collect::<Vec<_>>();
                axum::Json(serde_json::json!({"data": data}))
            }
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "secret-workspace-key".into(),
        model: model.into(),
        dimensions: width,
        send_dimensions_param: false,
        query_instruction: String::new(),
        document_prefix: String::new(),
    });
    (Arc::new(provider), server)
}

async fn record_delivery(ctx: &loco_rs::app::AppContext, workspace_id: Uuid) -> Uuid {
    yorishiro::workers::reindex::enqueue_reindex(ctx, workspace_id, None)
        .await
        .expect("enqueue reindex through production outbox")
        .parse()
        .expect("lifecycle UUID")
}

#[tokio::test]
async fn reindex_retries_when_the_provider_is_removed_during_embedding() {
    use axum::{Json, Router, http::StatusCode, routing::post};
    use loco_rs::bgworker::BackgroundWorker;
    use serde_json::{Value, json};
    use tokio::sync::Notify;
    use yorishiro::workers::reindex::{ReindexArgs, ReindexWorker};

    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let setup = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        ).await;
        let auth = format!("Bearer {}", setup.api_key);
        request.post("/api/schemas").add_header("Authorization", &auth)
            .json(&json!({"name":"reindex-test","entity_types":{"note":{"fields":{
                "title":{"type":"string","x-embed":true}
            }}}})).await.assert_status(StatusCode::CREATED);
        request.post("/api/entities").add_header("Authorization", &auth)
            .json(&json!({"schema_name":"reindex-test","entity_type":"note","data":{"title":"original"}}))
            .await.assert_status(StatusCode::CREATED);

        let started = Arc::new(Notify::new());
        let removed = Arc::new(Notify::new());
        let endpoint = crate::http_stub::HttpStub::start(Router::new().route("/embeddings", post({
            let (started, removed) = (started.clone(), removed.clone());
            move |Json(body): Json<Value>| {
                let (started, removed) = (started.clone(), removed.clone());
                async move {
                    if body["input"][0] == "title: original" {
                        started.notify_one();
                        removed.notified().await;
                    }
                    Json(json!({"data":[{"index":0,"embedding":[1.0,0.0,0.0,0.0]}]}))
                }
            }
        }))).await;
        let (fallback, _fallback_endpoint) = provider("fallback", 4, 1.0, Arc::new(Mutex::new(0))).await;
        ctx.shared_store.insert(fallback);
        request.put("/api/workspace/embedding-provider").add_header("Authorization", &auth)
            .json(&json!({"base_url":endpoint.url,"model":"override","api_key":"test-key","dimensions":4}))
            .await.assert_status(StatusCode::NO_CONTENT);
        let job = request.post("/api/migration-jobs/reindex").add_header("Authorization", &auth).await;
        job.assert_status(StatusCode::OK);
        let id = job.json::<Value>()["migration_job_id"].as_str().unwrap().parse::<Uuid>().unwrap();
        let worker = ReindexWorker::build(&ctx);
        let dispatch = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
            .one(&ctx.db).await.unwrap().unwrap();
        let args: ReindexArgs = serde_json::from_value(dispatch.payload).unwrap();
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(worker.perform(args), async {
                started.notified().await;
                request.delete("/api/workspace/embedding-provider").add_header("Authorization", &auth)
                    .await.assert_status(StatusCode::NO_CONTENT);
                removed.notify_one();
            })
        }).await.expect("the provider change and reindex finish");
        result.expect("the same reindex retries and completes");
        let status = request.get(&format!("/api/migration-jobs/{id}")).add_header("Authorization", &auth).await;
        status.assert_status(StatusCode::OK);
        assert_eq!(status.json::<Value>()["status"], "completed");
        let workspace = workspace_workspaces::Entity::find_by_id(setup.workspace_id)
            .one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(workspace.embedding_model.as_deref(), Some("fallback"));
        assert!(!workspace.embedding_reindexing);
    }).await;
}

#[tokio::test]
async fn worker_uses_workspace_provider_and_keeps_key_out_of_payload() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
    let crate::requests::fixtures::TestTenant { tenant_id, workspace_id, api_key, .. } = crate::requests::fixtures::TestTenant::create(&request, &ctx, crate::requests::fixtures::TenantArgs::default()).await;
    let default_calls = Arc::new(Mutex::new(0));
    let workspace_calls = Arc::new(Mutex::new(0));
    let (default, _default_server) =
        provider("deployment-model", 1536, 1.0, default_calls.clone()).await;
    let (_workspace_provider, workspace_server) =
        provider("workspace-model", 1024, 2.0, workspace_calls.clone()).await;
    schema_schemas::create_schema(
        &ctx.db,
        tenant_id,
        workspace_id,
        serde_json::from_value(serde_json::json!({"name":"note","entity_types":{"note":{"fields":{"title":{"type":"string","required":true,"x-embed":true}}}}})).expect("definition"),
        None,
        None,
    ).await.expect("schema");
    let entity = entity_entities::create(
        &ctx.db,
        workspace_id,
        CreateEntityInput {
            schema_name: "note".into(),
            entity_type: "note".into(),
            data: serde_json::json!({"title":"workspace provider candidate"}),
        },
        None,
    )
    .await
    .expect("entity");
    let configured = request
        .put("/api/workspace/embedding-provider")
        .add_header("Authorization", format!("Bearer {api_key}"))
        .json(&serde_json::json!({
            "base_url": workspace_server.url,
            "model": "workspace-model",
            "api_key": "secret-workspace-key",
            "dimensions": 1024,
            "send_dimensions_param": false,
        }))
        .await;
    configured.assert_status(axum::http::StatusCode::NO_CONTENT);
    let described = request
        .get("/api/workspace/embedding-provider")
        .add_header("Authorization", format!("Bearer {api_key}"))
        .await;
    described.assert_status(axum::http::StatusCode::OK);
    assert!(!described.json::<serde_json::Value>().to_string().contains("secret-workspace-key"));
    ctx.shared_store
        .insert(default as Arc<dyn EmbeddingProvider>);
    let lifecycle_id = yorishiro::models::queue_job_lifecycles::Entity::find()
        .filter(yorishiro::models::queue_job_lifecycles::Column::WorkspaceId.eq(workspace_id))
        .filter(yorishiro::models::queue_job_lifecycles::Column::JobName.eq("reindex"))
        .one(&ctx.db).await.unwrap().expect("configuration queues reindex").id;
    let outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(lifecycle_id)
        .one(&ctx.db).await.expect("read durable payload").expect("outbox exists");
    let encoded = outbox.payload.to_string();
    assert!(!encoded.contains("key") && !encoded.contains("secret"));
    crate::requests::jobs::finish_job(&ctx, lifecycle_id, "completed", None).await;
    let row = workspace_workspaces::Entity::find_by_id(workspace_id)
        .one(&ctx.db)
        .await
        .expect("workspace")
        .expect("row");
    assert_eq!(row.embedding_model.as_deref(), Some("workspace-model"));
    assert_eq!(row.embedding_dimensions, Some(1024));
    assert_eq!(*workspace_calls.lock().unwrap(), 2);
    assert_eq!(*default_calls.lock().unwrap(), 0);
    let backend = ctx.db.get_database_backend();
    let length = Func::cust(Alias::new(if backend == sea_orm::DatabaseBackend::Sqlite {
        "length"
    } else {
        "vector_dims"
    })).arg(Expr::col(entity_embeddings::Column::Embedding));
    let stored_length = entity_embeddings::Entity::find().filter(entity_embeddings::Column::EntityId.eq(entity.id))
        .select_only()
        .expr(length)
        .into_tuple::<i32>()
        .one(&ctx.db).await.expect("embedding row").expect("embedding exists");
    let bytes = if backend == sea_orm::DatabaseBackend::Sqlite { stored_length } else { stored_length * 4 };
    assert_eq!(bytes, 1024 * 4);
    }).await;
}

#[tokio::test]
async fn disabled_provider_is_a_terminal_worker_error() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let mut settings = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .expect("deployment settings");
        settings.embedding = serde_json::from_value(serde_json::json!({
            "provider": "none", "dimensions": 768, "api_key": "",
            "send_dimensions_param": false, "local_model": "unused",
            "local_max_sequence_length": 512
        }))
        .expect("disabled embedding settings");
        let provider = yorishiro::services::embedding::build_embedding_provider(&settings)
            .await
            .expect("build disabled production provider");
        ctx.shared_store.insert(provider);
        let expected_error = "no embedding provider is configured";
        let crate::requests::fixtures::TestTenant {
            workspace_id,
            api_key,
            ..
        } = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        let lifecycle_id = record_delivery(&ctx, workspace_id).await;
        crate::requests::jobs::finish_job(&ctx, lifecycle_id, "failed", None).await;
        let lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(lifecycle_id)
            .one(&ctx.db)
            .await
            .expect("read terminal lifecycle")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.status, "failed");
        assert!(
            lifecycle
                .error
                .as_deref()
                .is_some_and(|error| error.contains(expected_error)),
            "error={:?}, expected substring={expected_error}",
            lifecycle.error
        );
        let response = request
            .get(&format!("/api/migration-jobs/{lifecycle_id}"))
            .add_header("Authorization", format!("Bearer {api_key}"))
            .await;
        response.assert_status(axum::http::StatusCode::OK);
        assert_eq!(
            response.json::<serde_json::Value>()["error"],
            "No embedding provider is configured."
        );
    })
    .await;
}
