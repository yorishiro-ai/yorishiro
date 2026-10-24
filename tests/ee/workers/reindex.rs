use std::sync::{Arc, Mutex};

use sea_orm::{ConnectionTrait, EntityTrait, Statement};
use uuid::Uuid;

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
async fn worker_uses_workspace_provider_and_keeps_key_out_of_payload() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
    let crate::requests::fixtures::TestTenant { tenant_id, workspace_id, api_key, .. } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
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
        .put("/api/workspace/embedding-key")
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
        .get("/api/workspace/embedding-key")
        .add_header("Authorization", format!("Bearer {api_key}"))
        .await;
    described.assert_status(axum::http::StatusCode::OK);
    assert!(!described.json::<serde_json::Value>().to_string().contains("secret-workspace-key"));
    ctx.shared_store
        .insert(default as Arc<dyn EmbeddingProvider>);
    let lifecycle_id = record_delivery(&ctx, workspace_id).await;
    let outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(lifecycle_id)
        .one(&ctx.db).await.expect("read durable payload").expect("outbox exists");
    let encoded = outbox.payload.to_string();
    assert!(!encoded.contains("key") && !encoded.contains("secret"));
    crate::requests::fill_defaults::finish(&ctx, lifecycle_id).await;
    let row = workspace_workspaces::Entity::find_by_id(workspace_id)
        .one(&ctx.db)
        .await
        .expect("workspace")
        .expect("row");
    assert_eq!(row.embedding_model.as_deref(), Some("workspace-model"));
    assert_eq!(row.embedding_dimensions, Some(1024));
    assert_eq!(*workspace_calls.lock().unwrap(), 1);
    assert_eq!(*default_calls.lock().unwrap(), 0);
    let backend = ctx.db.get_database_backend();
    let sql = if backend == sea_orm::DatabaseBackend::Sqlite {
        "SELECT length(embedding) FROM entity_embeddings_1024 WHERE entity_id = $1"
    } else {
        "SELECT vector_dims(embedding) * 4 FROM entity_embeddings_1024 WHERE entity_id = $1"
    };
    let bytes: i32 = ctx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            backend,
            sql,
            [entity.id.into()],
        ))
        .await
        .expect("embedding row")
        .expect("embedding exists")
        .try_get_by_index(0)
        .expect("embedding length");
    assert_eq!(bytes, 1024 * 4);
    }).await;
}

#[tokio::test]
async fn disabled_provider_is_a_terminal_worker_error() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
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
        let expected_error = "embedding provider disabled";
        let crate::requests::fixtures::TestTenant { workspace_id, .. } =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let lifecycle_id = record_delivery(&ctx, workspace_id).await;
        crate::requests::fill_defaults::finish_with_status(&ctx, lifecycle_id, "failed").await;
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
    })
    .await;
}
