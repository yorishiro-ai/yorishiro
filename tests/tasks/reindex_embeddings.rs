use crate::requests::fixtures::{TenantArgs, TestTenant};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::requests::boot_request;
use sea_orm::{ColumnTrait, EntityTrait, QuerySelect};
use yorishiro::App;
use yorishiro::models::_entities::workspace_workspaces;
use yorishiro::models::entity_embeddings;
use yorishiro::models::{entity_entities, schema_schemas};
use yorishiro::services::embedding::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

fn note_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "note",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true, "x-embed": true } } }
        }
    })
}

async fn insert_workspace(
    request: &loco_rs::TestServer,
    ctx: &loco_rs::app::AppContext,
    stamped_model: &str,
) -> uuid::Uuid {
    let TestTenant {
        tenant_id,
        workspace_id,
        ..
    } = TestTenant::create(request, ctx, TenantArgs::default()).await;
    let (previous, _server) = http_provider(stamped_model, usize::MAX).await;
    entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &[], &previous)
        .await
        .expect("stamp the previous deployed model");
    let def = serde_json::from_value(note_definition()).expect("parse definition");
    schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
        .await
        .expect("create schema");
    workspace_id
}

async fn insert_entity(
    ctx: &loco_rs::app::AppContext,
    workspace_id: uuid::Uuid,
    title: &str,
) -> entity_entities::EntityRecord {
    entity_entities::create(
        &ctx.db,
        workspace_id,
        entity_entities::CreateEntityInput {
            schema_name: "note".into(),
            entity_type: "note".into(),
            data: serde_json::json!({ "title": title }),
        },
        None,
    )
    .await
    .expect("create entity")
}

async fn stamped_model(ctx: &loco_rs::app::AppContext, workspace_id: uuid::Uuid) -> Option<String> {
    workspace_workspaces::Entity::find_by_id(workspace_id)
        .select_only()
        .column(workspace_workspaces::Column::EmbeddingModel)
        .into_tuple::<Option<String>>()
        .one(&ctx.db)
        .await
        .expect("query stamp")
        .flatten()
}

async fn embedding_is_set(ctx: &loco_rs::app::AppContext, entity_id: uuid::Uuid) -> bool {
    entity_embeddings::Entity768::find_by_id(entity_id)
        .select_only()
        .column_as(
            yorishiro::models::_entities::entity_embeddings_768::Column::Embedding.is_not_null(),
            "has_embedding",
        )
        .into_tuple::<bool>()
        .one(&ctx.db)
        .await
        .expect("query embedding")
        .unwrap_or(false)
}

async fn http_provider(
    model: &str,
    succeeds_before_failing: usize,
) -> (OpenAiCompatibleProvider, crate::http_stub::HttpStub) {
    let calls = Arc::new(AtomicUsize::new(0));
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let calls = calls.clone();
            async move {
                if calls.fetch_add(1, Ordering::SeqCst) >= succeeds_before_failing {
                    return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
                }
                let data = body["input"]
                    .as_array()
                    .expect("embedding inputs")
                    .iter()
                    .enumerate()
                    .map(|(index, _)| serde_json::json!({"index": index, "embedding": vec![0.0_f32; 768]}))
                    .collect::<Vec<_>>();
                Ok(axum::Json(serde_json::json!({"data": data})))
            }
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: model.into(),
        dimensions: 768,
        send_dimensions_param: false,
    });
    (provider, server)
}

/// A partial failure must leave the workspace's stamp completely unchanged, which is the load-bearing property `reindex_embeddings` exists to guarantee: restamping on a partial result would claim a model for rows that were never actually re-embedded with it, recreating the exact stamp/data mismatch the write-time model check in `sync.rs` exists to catch.
#[tokio::test]
async fn reindex_workspace_leaves_the_stamp_unchanged_on_partial_failure() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let workspace_id = insert_workspace(&request, &ctx, "nomic-ai/nomic-embed-text-v1.5").await;
        let first = insert_entity(&ctx, workspace_id, "first entity").await;
        let second = insert_entity(&ctx, workspace_id, "second entity").await;

        let (provider, _server) = http_provider("intfloat/multilingual-e5-base", 1).await;
        let outcome = yorishiro::db::reindex_workspace_with_lock(
            &ctx,
            workspace_id,
            &[first.id, second.id],
            &provider,
        )
        .await
        .expect("reindex_workspace runs even with per-entity failures");

        assert_eq!(outcome.total, 2);
        assert_eq!(
            outcome.reindexed, 1,
            "exactly one entity should have embedded before the simulated outage"
        );
        assert_eq!(outcome.failures.len(), 1);

        assert_eq!(
            stamped_model(&ctx, workspace_id).await,
            Some("nomic-ai/nomic-embed-text-v1.5".into()),
            "a partial failure must not touch the workspace's stamp"
        );
    })
    .await;
}

/// Full success must both write every row's vector and restamp the workspace to the new provider's identity, which is what lets a subsequent ordinary write pass the write-time model check instead of being refused forever.
#[tokio::test]
async fn reindex_workspace_restamps_only_after_every_entity_succeeds() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let workspace_id = insert_workspace(&request, &ctx, "nomic-ai/nomic-embed-text-v1.5").await;
        let first = insert_entity(&ctx, workspace_id, "first entity").await;
        let second = insert_entity(&ctx, workspace_id, "second entity").await;

        let (provider, _server) = http_provider("intfloat/multilingual-e5-base", usize::MAX).await;
        let outcome = yorishiro::db::reindex_workspace_with_lock(
            &ctx,
            workspace_id,
            &[first.id, second.id],
            &provider,
        )
        .await
        .expect("reindex_workspace succeeds");

        assert_eq!(outcome.total, 2);
        assert_eq!(outcome.reindexed, 2);
        assert!(outcome.failures.is_empty());

        assert!(embedding_is_set(&ctx, first.id).await);
        assert!(embedding_is_set(&ctx, second.id).await);
        assert_eq!(
            stamped_model(&ctx, workspace_id).await,
            Some("intfloat/multilingual-e5-base".into()),
            "full success must restamp the workspace to the provider that actually wrote the vectors"
        );

    })
    .await;
}

/// An entity modified between `reindex_workspace`'s batch fetch and its write landing must be reported as a failure, not silently counted as reindexed, or the workspace would restamp while that row still holds a vector from before the concurrent modification: the exact stamp/data mismatch the write-time model check exists to catch, this time caused by the migration tool racing an ordinary write instead of a misconfigured deployment.
#[tokio::test]
async fn reindex_workspace_reports_a_concurrently_modified_entity_as_a_failure() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let workspace_id = insert_workspace(&request, &ctx, "nomic-ai/nomic-embed-text-v1.5").await;
        let first = insert_entity(&ctx, workspace_id, "first entity").await;
        let second = insert_entity(&ctx, workspace_id, "second entity").await;

        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let (started, gate) = (entered.clone(), release.clone());
        let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
            "/embeddings",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let (started, gate, calls) = (started.clone(), gate.clone(), calls.clone());
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        started.notify_one();
                        gate.acquire().await.unwrap().forget();
                    }
                    let data = body["input"].as_array().unwrap().iter().enumerate()
                        .map(|(index, _)| serde_json::json!({"index": index, "embedding": vec![0.0_f32; 768]}))
                        .collect::<Vec<_>>();
                    axum::Json(serde_json::json!({"data": data}))
                }
            }),
        )).await;
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "test-key".into(),
            model: "intfloat/multilingual-e5-base".into(),
            dimensions: 768,
            send_dimensions_param: false,
        });
        let ids = [first.id, second.id];
        let reindex = yorishiro::db::reindex_workspace_with_lock(
            &ctx,
            workspace_id,
            &ids,
            &provider,
        );
        let modify = async {
            entered.notified().await;
            entity_entities::update(
                &ctx.db,
                workspace_id,
                entity_entities::UpdateEntityInput {
                    id: first.id,
                    data: serde_json::json!({"title": "modified concurrently with the reindex"}),
                    updated_by: None,
                },
            ).await.expect("concurrent update");
            release.add_permits(1);
        };
        let (outcome, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(reindex, modify)
        })
        .await
        .expect("concurrent reindex completes");
        let outcome = outcome
        .expect("reindex_workspace runs even with a concurrent modification");

        assert_eq!(outcome.total, 2);
        assert_eq!(
            outcome.reindexed, 1,
            "only the entity that was not concurrently modified should count as reindexed"
        );
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].entity_id, first.id);

        assert_eq!(
            stamped_model(&ctx, workspace_id).await,
            Some("nomic-ai/nomic-embed-text-v1.5".into()),
            "a concurrently modified entity must block the restamp exactly like any other failure"
        );
    })
    .await;
}
