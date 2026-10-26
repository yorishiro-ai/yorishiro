use crate::requests::boot_request;
#[cfg(feature = "enterprise")]
use sea_orm::ConnectionTrait;
use sea_orm::sea_query::ExprTrait;
use sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter, QuerySelect, SelectExt,
    Statement,
};
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::entity_embeddings;
use yorishiro::models::{entity_entities, schema_schemas, search};
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

fn note_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "note",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true, "x-embed": true } } }
        }
    })
}

/// Vector search must rank by cosine distance (closest first) and must not leak another workspace's entities into the results, even when that workspace's vector is a closer match.
#[tokio::test]
async fn search_by_vector_ranks_by_distance_and_stays_within_the_workspace() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let txn = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
        let other_workspace = yorishiro::models::tenancy::create_workspace(
            &txn,
            tenant_id,
            &format!("other-{}", uuid::Uuid::new_v4()),
            None,
            None,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();

        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");
        let other_def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(
            &ctx.db,
            tenant_id,
            other_workspace.id,
            other_def,
            None,
            None,
        )
        .await
        .expect("create schema in other workspace");

        let close = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "close match" }),
            },
            None,
        )
        .await
        .expect("create close entity");
        let far = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "far match" }),
            },
            None,
        )
        .await
        .expect("create far entity");
        let other_workspace_entity = entity_entities::create(
            &ctx.db,
            other_workspace.id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "belongs to another workspace" }),
            },
            None,
        )
        .await
        .expect("create entity in other workspace");

        fn unit_vector(axis: usize) -> Vec<f32> {
            let mut v = vec![0.0_f32; 768];
            v[axis] = 1.0;
            v
        }

        let query_vector = unit_vector(0);
        let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
            "/embeddings",
            axum::routing::post(
                |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    let data = body["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                        .map(|(index, text)| {
                            let axis = usize::from(text.as_str().unwrap().contains("far match"));
                            serde_json::json!({"index": index, "embedding": unit_vector(axis)})
                        })
                        .collect::<Vec<_>>();
                    axum::Json(serde_json::json!({"data": data}))
                },
            ),
        ))
        .await;
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "test-key".into(),
            model: "ranking".into(),
            dimensions: 768,
            send_dimensions_param: false,
        });
        for (workspace, entity) in [
            (workspace_id, close.id),
            (workspace_id, far.id),
            (other_workspace.id, other_workspace_entity.id),
        ] {
            let record = entity_entities::get(&ctx.db, workspace, entity)
                .await
                .unwrap();
            entity_embeddings::sync_embedding_for_record(&ctx.db, workspace, &record, &provider)
                .await
                .unwrap();
        }

        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            query_vector,
            "match",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector");

        assert_eq!(hits.len(), 2, "hits: {hits:?}");
        assert_eq!(hits[0].entity.id, close.id, "closest vector ranks first");
        assert_eq!(hits[1].entity.id, far.id);
        assert!(hits[0].distance.unwrap() < hits[1].distance.unwrap());
        assert!(
            hits.iter().all(|h| h.entity.workspace_id == workspace_id),
            "no cross-workspace leakage: {hits:?}"
        );
    })
    .await;
}

async fn fixed_width_provider(
    width: usize,
) -> (OpenAiCompatibleProvider, crate::http_stub::HttpStub) {
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| async move {
            let data = body["input"].as_array().unwrap().iter().enumerate()
                .map(|(index, _)| serde_json::json!({"index": index, "embedding": vec![1.0_f32; width]}))
                .collect::<Vec<_>>();
            axum::Json(serde_json::json!({"data": data}))
        }),
    )).await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: "fixed-width".into(),
        dimensions: width,
        send_dimensions_param: false,
    });
    (provider, server)
}

fn model_vector(model: &str) -> Vec<f32> {
    let mut vector = vec![0.0; 768];
    for (value, byte) in vector.iter_mut().zip(model.bytes()) {
        *value = byte as f32 / 255.0;
    }
    vector
}

async fn model_provider(
    model: &'static str,
) -> (OpenAiCompatibleProvider, crate::http_stub::HttpStub) {
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| async move {
            let data = body["input"].as_array().unwrap().iter().enumerate()
                .map(|(index, _)| serde_json::json!({"index": index, "embedding": model_vector(model)}))
                .collect::<Vec<_>>();
            axum::Json(serde_json::json!({"data": data}))
        }),
    )).await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: model.into(),
        dimensions: 768,
        send_dimensions_param: false,
    });
    (provider, server)
}

/// A workspace stamped with a dimension count (`workspace_workspaces.embedding_dimensions`) must refuse a sync whose provider produces a different width, rather than writing a vector that would silently break every future search over that workspace with a dimension-mismatch error naming neither the entity nor the write that caused it.
#[tokio::test]
async fn sync_embedding_refuses_a_vector_that_does_not_match_the_workspace_stamp() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let (stamp_provider, _stamp_server) = fixed_width_provider(768).await;
        entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &stamp_provider)
            .await
            .expect("stamp workspace through reindex");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "will not get an embedding" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("get entity record");

        let (mismatched_provider, _server) = fixed_width_provider(1024).await;
        let result = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            workspace_id,
            &record,
            &mismatched_provider,
        )
        .await;

        assert!(
            matches!(result, Err(YorishiroError::ValidationFailed { .. })),
            "result: {result:?}"
        );

        let stored = entity_embeddings::Entity::find_by_id(entity.id)
            .filter(
                yorishiro::models::_entities::entity_embeddings::Column::Embedding.is_not_null(),
            )
            .exists(&ctx.db)
            .await
            .expect("query embedding");
        assert!(
            !stored,
            "the refused write must not have created an embedding row: {stored:?}"
        );
    })
    .await;
}

async fn gated_model_provider(
    model: &'static str,
) -> (
    std::sync::Arc<OpenAiCompatibleProvider>,
    crate::http_stub::HttpStub,
    std::sync::Arc<tokio::sync::Notify>,
    std::sync::Arc<tokio::sync::Semaphore>,
) {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let handler_entered = entered.clone();
    let handler_release = release.clone();
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let calls = calls.clone();
            let entered = handler_entered.clone();
            let release = handler_release.clone();
            async move {
                let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if call == 1 {
                    entered.notify_one();
                    release.acquire().await.expect("release embedding response").forget();
                }
                let data = body["input"].as_array().unwrap().iter().enumerate()
                    .map(|(index, _)| serde_json::json!({"index": index, "embedding": model_vector(model)}))
                    .collect::<Vec<_>>();
                axum::Json(serde_json::json!({"data": data}))
            }
        }),
    )).await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: model.into(),
        dimensions: 768,
        send_dimensions_param: false,
    });
    (std::sync::Arc::new(provider), server, entered, release)
}

/// A workspace stamped with a model name (`workspace_workspaces.embedding_model`) must refuse a sync whose provider reports a different model, even though both produce 768-dimensional vectors and the dimension check above cannot see any difference: this is exactly the nomic/multilingual-e5-base coexistence `entity_entities.embedding vector(768)` allows, and the case that check exists to catch.
#[tokio::test]
async fn sync_embedding_refuses_a_vector_from_a_different_model_than_the_workspace_stamp() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let (stamp_provider, _stamp_server) =
            model_provider("nomic-ai/nomic-embed-text-v1.5").await;
        entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &stamp_provider)
            .await
            .expect("stamp workspace through reindex");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "will not get an embedding" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("get entity record");

        let (mismatched_provider, _mismatched_provider_server) =
            model_provider("intfloat/multilingual-e5-base").await;
        let result = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            workspace_id,
            &record,
            &mismatched_provider,
        )
        .await;

        assert!(
            matches!(result, Err(YorishiroError::ValidationFailed { .. })),
            "result: {result:?}"
        );

        let stored = entity_embeddings::Entity::find_by_id(entity.id)
            .filter(
                yorishiro::models::_entities::entity_embeddings::Column::Embedding.is_not_null(),
            )
            .exists(&ctx.db)
            .await
            .expect("query embedding");
        assert!(
            !stored,
            "the refused write must not have created an embedding row: {stored:?}"
        );
    })
    .await;
}

/// A workspace with no stamp of its own inherits its tenant's `embedding_model`/`embedding_dimensions` as the effective model check target and dimension target.
/// If the tenant's value and the deployment default are the same, deleting the join would pass this test, so the tenant is set to a different model than the deployment default: removing the join would then cause the test to use the deployment default instead of the tenant's, and the provider mismatch would be caught by the model check.
#[tokio::test]
async fn sync_embedding_resolves_the_tenant_tier_of_the_embedding_chain() {
    if !super::super::require_postgres_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        // No public operation sets tenant embedding stamps; inject the legacy fallback tier.
        sea_orm::ActiveModelTrait::update(
            tenant_tenants::ActiveModel {
                id: sea_orm::ActiveValue::Set(fixture.tenant_id),
                embedding_model: sea_orm::ActiveValue::Set(Some(
                    "nomic-ai/nomic-embed-text-v1.5".into(),
                )),
                embedding_dimensions: sea_orm::ActiveValue::Set(Some(768)),
                ..Default::default()
            },
            &ctx.db,
        )
        .await
        .expect("stamp tenant embedding fallback");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            def,
            None,
            None,
        )
        .await
        .expect("create schema");

        let entity = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "resolves from tenant tier" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let record = entity_entities::get(&ctx.db, fixture.workspace_id, entity.id)
            .await
            .expect("get entity record");

        // The tenant is stamped with nomic-embed-text-v1.5, so the effective model must be that.
        // Using the same model provider should succeed.
        let (matching_provider, _matching_provider_server) =
            model_provider("nomic-ai/nomic-embed-text-v1.5").await;
        let result = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            fixture.workspace_id,
            &record,
            &matching_provider,
        )
        .await;
        assert!(result.is_ok(), "result: {result:?}");

        // Verify the embedding was written.
        let stored = entity_embeddings::Entity::find_by_id(entity.id)
            .filter(
                yorishiro::models::_entities::entity_embeddings::Column::Embedding.is_not_null(),
            )
            .exists(&ctx.db)
            .await
            .expect("query embedding");
        assert!(stored, "embedding must have been written");

        // A different model should be refused by the inherited model check.
        let entity2 = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "wrong model, should be refused" }),
            },
            None,
        )
        .await
        .expect("create second entity");
        let record2 = entity_entities::get(&ctx.db, fixture.workspace_id, entity2.id)
            .await
            .expect("get entity record");

        let (mismatched_provider, _mismatched_provider_server) =
            model_provider("intfloat/multilingual-e5-base").await;
        let result2 = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            fixture.workspace_id,
            &record2,
            &mismatched_provider,
        )
        .await;
        assert!(
            matches!(result2, Err(YorishiroError::ValidationFailed { .. })),
            "result: {result2:?}"
        );
    })
    .await;
}

/// A workspace inheriting its tenant's `embedding_dimensions` must refuse a sync whose provider
/// produces a different width. The dimension check runs before the model check, so a 768 provider
/// against a 1024 tenant is rejected on width; the `contains("1024")` assertion in the test
/// proves it was the dimension and not the model check that fired.
#[tokio::test]
async fn sync_embedding_resolves_the_tenant_dimension_tier() {
    if !super::super::require_postgres_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        // No public operation sets tenant embedding stamps; inject the legacy fallback tier.
        sea_orm::ActiveModelTrait::update(
            tenant_tenants::ActiveModel {
                id: sea_orm::ActiveValue::Set(fixture.tenant_id),
                embedding_model: sea_orm::ActiveValue::Set(Some(
                    "nomic-ai/nomic-embed-text-v1.5".into(),
                )),
                embedding_dimensions: sea_orm::ActiveValue::Set(Some(1024)),
                ..Default::default()
            },
            &ctx.db,
        )
        .await
        .expect("stamp tenant embedding fallback");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            def,
            None,
            None,
        )
        .await
        .expect("create schema");

        let entity = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "wrong dimension from tenant" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let record = entity_entities::get(&ctx.db, fixture.workspace_id, entity.id)
            .await
            .expect("get entity record");
        // Provider is 768-dimensional, not the workspace's 1024: the dimension check fires
        // first and rejects the write before the model check is reached.
        let (provider, _server) = fixed_width_provider(768).await;
        let result = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            fixture.workspace_id,
            &record,
            &provider,
        )
        .await;

        assert!(
            matches!(result, Err(YorishiroError::ValidationFailed { .. })),
            "expected ValidationFailed for dimension mismatch: {result:?}"
        );
        assert!(
            result.unwrap_err().to_string().contains("1024"),
            "error message must name the expected dimension count 1024"
        );
    })
    .await;
}

/// Two concurrent `reindex_workspace_with_lock` calls against the same workspace with different
/// providers must serialize via the advisory lock: the second waits for the first, and the final
/// stamp and per-row vector provenance agree.
///
/// Without the lock acquisition in `src/db.rs`, the two concurrent calls race and the final stamp does
/// not match the vectors actually stored, causing this assertion to fail.
#[tokio::test]
async fn concurrent_reindex_runs_serialize_and_consistent_after_lock() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant { tenant_id, workspace_id, .. } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        // Create entities to reindex.
        let e1 = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity one" }),
            },
            None,
        )
        .await
        .expect("create entity 1");
        let e2 = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity two" }),
            },
            None,
        )
        .await
        .expect("create entity 2");

        // Pre-stamp with an old model to simulate a workspace that needs reindexing.
        let (stamp_provider, _stamp_server) = model_provider("old-model").await;
        entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &stamp_provider)
            .await
            .expect("stamp workspace through reindex");

        let candidate_ids: Vec<uuid::Uuid> = vec![e1.id, e2.id];

        let (provider1, _provider1_server, entered, release) =
            gated_model_provider("nomic-ai/nomic-embed-text-v1.5").await;
        let (provider2, _provider2_server) =
            model_provider("intfloat/multilingual-e5-base").await;
        let provider2 = std::sync::Arc::new(provider2);

        let provider1_clone = provider1.clone();
        let ctx1 = ctx.clone();
        let h1 = tokio::spawn(async move {
            yorishiro::db::reindex_workspace_with_lock(
                &ctx1,
                workspace_id,
                provider1_clone.as_ref(),
            )
            .await
            .expect("reindex 1 join")
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
            .await.expect("first reindex reached its second embedding request");
        let ctx2 = ctx.clone();
        let provider2_clone = provider2.clone();
        let h2 = tokio::spawn(async move {
            yorishiro::db::reindex_workspace_with_lock(
                &ctx2,
                workspace_id,
                provider2_clone.as_ref(),
            )
            .await
            .expect("reindex 2 join")
        });

        // Inspect the actual lock wait, not elapsed time; this is a database-lock regression.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                #[derive(sea_orm::FromQueryResult)]
                struct LockWait { waiting: bool }
                let waiting = LockWait::find_by_statement(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND NOT granted AND classid::bigint = ((hashtextextended($1, 0) >> 32) & 4294967295) AND objid::bigint = (hashtextextended($1, 0) & 4294967295) AND objsubid = 1) AS waiting",
                    [yorishiro::db::workspace_reindex_lock_key(workspace_id).into()],
                )).one(&ctx.db).await.expect("observe workspace advisory lock")
                    .expect("lock observation row").waiting;
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.expect("second reindex waits for this workspace's advisory lock");
        release.add_permits(1);

        let outcome1 = h1.await.expect("handler 1 finished");
        let outcome2 = h2.await.expect("handler 2 finished");

        assert!(
            outcome1.failures.is_empty(),
            "first reindex failed: {}",
            outcome1
                .failures
                .iter()
                .map(|f| f.error.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert!(
            outcome2.failures.is_empty(),
            "second reindex failed: {}",
            outcome2
                .failures
                .iter()
                .map(|f| f.error.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // The final workspace stamp must match the provider that won the lock race.
        // We check that the stamp and the stored vectors agree: whichever provider won,
        // both the stamp and every entity's embedding must point to it.
        let final_model = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("query final workspace")
            .expect("workspace exists")
            .embedding_model
            .expect("workspace must be stamped with a model");

        let winner = if final_model == "nomic-ai/nomic-embed-text-v1.5" {
            provider1.as_ref()
        } else if final_model == "intfloat/multilingual-e5-base" {
            provider2.as_ref()
        } else {
            panic!("unexpected model stamp: {final_model:?}");
        };

        // Verify every entity's embedding matches the winning provider's vector.
        // `PgVector::data()` returns the raw `Vec<f32>` bytes.
        // The float comparison is safe: `*b as f32 / 255.0` round-trips exactly through
        // PostgreSQL `real` (32-bit float), so `assert_eq!` is valid.
        for entity_id in &candidate_ids {
            let stored_vector: Option<sea_orm::entity::prelude::PgVector> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    embedding: Option<sea_orm::entity::prelude::PgVector>,
                }
                entity_embeddings::Entity::find_by_id(*entity_id)
                    .select_only()
                    .column(yorishiro::models::_entities::entity_embeddings::Column::Embedding)
                    .into_model::<Row>()
                    .one(&ctx.db)
                    .await
                    .expect("query embedding")
                    .and_then(|r| r.embedding)
            };
            let expected = model_vector(&winner.model_name());
            assert_eq!(
                stored_vector.as_ref().map(|v| v.as_slice()),
                Some(expected.as_slice()),
                "entity {entity_id} embedding must match winner {final_model:?}"
            );
        }
    })
    .await;
}

/// `reindex_workspace` must overwrite existing vectors with the new model's embeddings: if a
/// provider's `embed_batch` impl is replaced and a reindex runs, every entity's embedding must
/// change from the old model's vector to the new model's vector.
///
/// This proves both halves: `assert_ne!` against the old model's vector (the row was not left
/// untouched) and `assert_eq!` against the new model's vector (the new embedding was actually
/// written). Without the `assert_ne!`, a broken `reindex` that skipped `embed_as` and left
/// the old vector in place would still pass the `assert_eq!` on the existing data; without the
/// `assert_eq!`, a corrupted or zeroed row would pass `assert_ne!` on its own.
#[tokio::test]
async fn reindex_overwrites_existing_entity_embeddings() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        // Create entities.
        let e1 = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity one" }),
            },
            None,
        )
        .await
        .expect("create entity 1");
        let e2 = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity two" }),
            },
            None,
        )
        .await
        .expect("create entity 2");

        // Embed with the old model provider.
        let (old_provider, _old_provider_server) = model_provider("old-model-a").await;
        for entity in [&e1, &e2] {
            let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
                .await
                .expect("get entity record");
            entity_embeddings::sync_embedding_for_record(
                &ctx.db,
                workspace_id,
                &record,
                &old_provider,
            )
            .await
            .expect("embed with old model");
        }

        // Verify the old vectors are stored.
        let old_v1 = model_vector(&old_provider.model_name());
        for entity_id in [e1.id, e2.id] {
            let stored: Option<sea_orm::entity::prelude::PgVector> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    embedding: Option<sea_orm::entity::prelude::PgVector>,
                }
                entity_embeddings::Entity::find_by_id(entity_id)
                    .select_only()
                    .column(yorishiro::models::_entities::entity_embeddings::Column::Embedding)
                    .into_model::<Row>()
                    .one(&ctx.db)
                    .await
                    .expect("query embedding")
                    .and_then(|r| r.embedding)
            };
            assert_eq!(
                stored.as_ref().map(|v| v.as_slice()),
                Some(old_v1.as_slice()),
                "initial embedding must match the old model"
            );
        }

        // Stamp the workspace with the old model so reindex considers it "in sync" — but
        // reindex's entire purpose is to re-embed regardless, so this stamp is only there
        // to make the test realistic.
        let (stamp_provider, _stamp_server) = model_provider("old-model-a").await;
        entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &stamp_provider)
            .await
            .expect("stamp workspace through reindex");

        // Reindex with the new model provider.
        let (new_provider, _new_provider_server) = model_provider("new-model-b").await;
        let outcome = yorishiro::models::entity_embeddings::reindex_workspace(
            &ctx.db,
            workspace_id,
            &new_provider,
        )
        .await
        .expect("reindex");

        if !outcome.failures.is_empty() {
            panic!("reindex had {} failure(s)", outcome.failures.len());
        }
        assert_eq!(outcome.reindexed, 2, "both entities must be reindexed");

        // Stamp and vectors must now agree on the new model.
        let final_model = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("query final workspace")
            .expect("workspace exists")
            .embedding_model
            .expect("workspace must be stamped with a model");
        assert_eq!(
            final_model, "new-model-b",
            "workspace stamp must reflect new model"
        );

        let new_v = model_vector(&new_provider.model_name());

        for entity_id in [e1.id, e2.id] {
            let stored: Option<sea_orm::entity::prelude::PgVector> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    embedding: Option<sea_orm::entity::prelude::PgVector>,
                }
                entity_embeddings::Entity::find_by_id(entity_id)
                    .select_only()
                    .column(yorishiro::models::_entities::entity_embeddings::Column::Embedding)
                    .into_model::<Row>()
                    .one(&ctx.db)
                    .await
                    .expect("query embedding")
                    .and_then(|r| r.embedding)
            };
            assert_eq!(
                stored.as_ref().map(|v| v.as_slice()),
                Some(new_v.as_slice()),
                "entity {entity_id} embedding must match new model {final_model:?}"
            );
        }
    })
    .await;
}

/// The trigram fallback surfaces an entity with no embedding at all, when its data fuzzy-matches `query_text`; an entity with neither an embedding nor a fuzzy match must not appear.
#[tokio::test]
async fn search_by_vector_falls_back_to_trigram_for_unembedded_entities() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        let matching = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "quarterly roadmap review" }),
            },
            None,
        )
        .await
        .expect("create matching entity");
        entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "completely unrelated grocery list" }),
            },
            None,
        )
        .await
        .expect("create unrelated entity");
        // Neither entity has an embedding: both rely entirely on the trigram fallback.

        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            vec![0.0_f32; 768],
            "quarterly roadmap",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector");

        assert_eq!(hits.len(), 1, "hits: {hits:?}");
        assert_eq!(hits[0].entity.id, matching.id);
        assert!(
            hits[0].distance.is_none(),
            "trigram-only hit has no distance"
        );
    })
    .await;
}

/// The FTS5 fallback on SQLite surfaces an entity whose `data` fuzzy-matches `query_text`, when
/// the entity has no embedding; an entity with neither an embedding nor a MATCH match must not
/// appear.
///
/// On SQLite, the search function's trigram half (pg_trgm on PostgreSQL) is replaced by an
/// FTS5 query against `entity_fts` with `tokenize='trigram'`. This test boots
/// against a SQLite file database to confirm the FTS5 path works end to end, including schema
/// creation, entity insertion, and trigger-driven index updates.
#[tokio::test]
async fn search_by_vector_falls_back_to_fts5_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }

    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        // Create an entity whose title contains the search phrase.
        let matching = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "quarterly roadmap review" }),
            },
            None,
        )
        .await
        .expect("create matching entity");

        // Create an entity whose title does not match.
        entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "completely unrelated grocery list" }),
            },
            None,
        )
        .await
        .expect("create unrelated entity");

        // Neither entity has an embedding (SQLite has no embedding column), so both rely on
        // the FTS5 fallback path.
        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            vec![0.0_f32; 768],
            "quarterly roadmap",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector");

        assert_eq!(hits.len(), 1, "hits: {hits:?}");
        assert_eq!(hits[0].entity.id, matching.id);
        assert!(hits[0].distance.is_none(), "FTS5-only hit has no distance");

        // Modify the entity's data so the old search phrase no longer matches, then
        // confirm a search for the new phrase finds it.
        let rec = entity_entities::get(&ctx.db, workspace_id, matching.id)
            .await
            .expect("fetch entity for update");

        entity_entities::update(
            &ctx.db,
            workspace_id,
            entity_entities::UpdateEntityInput {
                id: rec.id,
                data: serde_json::json!({ "title": "quarterly board meeting notes" }),
                updated_by: None,
            },
        )
        .await
        .expect("update entity");

        // Old phrase should no longer match.
        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            vec![0.0_f32; 768],
            "quarterly roadmap",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector after update");
        assert_eq!(
            hits.len(),
            0,
            "old phrase must not match after update: {hits:?}"
        );

        // New phrase should find it.
        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            vec![0.0_f32; 768],
            "quarterly board meeting",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector after update");
        assert_eq!(hits.len(), 1, "new phrase must match: {hits:?}");
        assert_eq!(hits[0].entity.id, matching.id);

        entity_entities::delete(&ctx.db, workspace_id, matching.id)
            .await
            .expect("delete entity");

        let after_delete = entity_entities::get(&ctx.db, workspace_id, matching.id).await;
        assert!(after_delete.is_err(), "deleted entity must not be found");
    })
    .await;
}

/// SQLite end-to-end: entity create → document embed via test provider → persisted vector → vector search hit.
///
/// Drives the public `sync_embedding_for_record` directly, not the worker.
/// `tests/requests/search.rs` covers the worker and the HTTP search on both backends.
#[tokio::test]
async fn sqlite_document_embed_persist_and_search() {
    if !super::super::require_sqlite_backend() {
        return;
    }

    boot_request::<App, _, _>(|request, ctx| async move {
            let crate::requests::fixtures::TestTenant { tenant_id, workspace_id, .. } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;

            let def = serde_json::from_value(note_definition()).expect("parse definition");
            schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
                .await
                .expect("create schema");

            // Create entity to embed.
            let entity = entity_entities::create(
                &ctx.db,
                workspace_id,
                entity_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: serde_json::json!({ "title": "quarterly roadmap review" }),
                },
                None,
            )
            .await
            .expect("create entity");

            // Exercise the production HTTP provider and persistence path.
            let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
                .await
                .expect("get entity record");
            let (provider, _provider_server) = model_provider("test-model").await;
            entity_embeddings::sync_embedding_for_record(
                &ctx.db,
                workspace_id,
                &record,
                &provider,
            )
            .await
            .expect("embed entity");

            // Verify the embedding was written to the shared table.
            let has_embedding = entity_embeddings::Entity::find_by_id(entity.id)
                .select_only()
                .expr_as(
                    sea_orm::sea_query::Expr::col(
                        yorishiro::models::_entities::entity_embeddings::Column::Embedding,
                    ).is_not_null(),
                    "has_embedding",
                )
                .into_tuple::<bool>()
                .one(&ctx.db)
                .await
                .expect("query embedding");
            assert_eq!(has_embedding, Some(true), "embedding must have been written");

            // Search by vector should find the entity.
            let hits = search::search_by_vector(
                &ctx.db,
                workspace_id,
                model_vector(&provider.model_name()),
                "quarterly roadmap",
                search::SearchQuery::default(),
            )
            .await
            .expect("search_by_vector");
            assert_eq!(hits.len(), 1, "vector search must find the embedded entity, hits: {hits:?}");
            assert_eq!(hits[0].entity.id, entity.id);
            assert!(
                hits[0].distance.is_some(),
                "vector hit must have a distance"
            );
            #[cfg(feature = "enterprise")]
            {
                use loco_rs::task::{Task, Vars};
                use yorishiro::edition::ee::tasks::sqlite_ann_benchmark::SqliteAnnBenchmark;
                SqliteAnnBenchmark.run(&ctx, &Vars::from_cli_args(vec![
                    ("workspace_id".into(), workspace_id.to_string()),
                ])).await.expect("benchmark stored embedding without a vec0 table");
                ctx.db.execute_unprepared("CREATE VIRTUAL TABLE entity_embeddings_vec0 USING vec0(entity_id TEXT PRIMARY KEY, workspace_id TEXT PARTITION KEY, embedding FLOAT[768] distance_metric=cosine)")
                    .await.unwrap();
                let unrelated_workspace = uuid::Uuid::now_v7();
                use sea_orm::sea_query::{Alias, Expr, Query};
                // Fault injection populates the optional vec0 index, not application-owned rows.
                for (id, workspace) in (0..11)
                    .map(|_| (uuid::Uuid::now_v7(), unrelated_workspace))
                    .chain(std::iter::once((entity.id, workspace_id)))
                {
                    let insert = Query::insert()
                        .into_table(Alias::new("entity_embeddings_vec0"))
                        .columns([Alias::new("entity_id"), Alias::new("workspace_id"), Alias::new("embedding")])
                        .values_panic([
                            id.to_string().into(), workspace.to_string().into(),
                            yorishiro::db::sqlite_vec_blob(&model_vector(&provider.model_name())).into(),
                        ]).to_owned();
                    ctx.db.execute_raw(ctx.db.get_database_backend().build(&insert)).await.unwrap();
                }
                SqliteAnnBenchmark.run(&ctx, &Vars::from_cli_args(vec![
                    ("workspace_id".into(), workspace_id.to_string()),
                    ("min_entities".into(), "0".into()),
                    ("max_latency_ms".into(), "0".into()),
                ])).await.expect("benchmark real vec0 KNN with UUID keys and workspace partition");
                let delete = Query::delete()
                    .from_table(Alias::new("entity_embeddings_vec0"))
                    .and_where(Expr::col(Alias::new("entity_id")).eq(entity.id.to_string()))
                    .to_owned();
                ctx.db.execute_raw(ctx.db.get_database_backend().build(&delete)).await.unwrap();
                let error = SqliteAnnBenchmark.run(&ctx, &Vars::from_cli_args(vec![
                    ("workspace_id".into(), workspace_id.to_string()),
                    ("min_entities".into(), "0".into()),
                    ("max_latency_ms".into(), "0".into()),
                ])).await.expect_err("an unpopulated workspace partition must not look like a fast benchmark");
                assert!(error.to_string().contains("returned no entities"), "{error}");
            }
    }).await;
}

/// A reindex re-embeds entities that already have a vector, which is the whole point of switching models.
/// Backend-neutral: runs on whichever backend `DATABASE_URL` names.
#[tokio::test]
async fn reindex_replaces_existing_vectors_and_restamps_the_workspace() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");
        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "already embedded" }),
            },
            None,
        )
        .await
        .expect("create entity");

        let (first, _first_server) = model_provider("model-a").await;
        let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("get entity");
        entity_embeddings::sync_embedding_for_record(&ctx.db, workspace_id, &record, &first)
            .await
            .expect("first embed");

        let (second, _second_server) = model_provider("model-b").await;
        let outcome = entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &second)
            .await
            .expect("reindex");

        assert_eq!(outcome.total, 1);
        assert_eq!(outcome.reindexed, 1);
        assert!(
            outcome.failures.is_empty(),
            "an entity that already has a vector must be reindexed, not reported missing"
        );
        let stamped = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("read workspace")
            .expect("workspace row");
        assert_eq!(stamped.embedding_model.as_deref(), Some("model-b"));
    })
    .await;
}

/// Width switching uses the actual vectors rather than a fixed registry of storage tables.
#[tokio::test]
async fn reindex_switches_between_arbitrary_widths() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            serde_json::from_value(note_definition()).unwrap(),
            None,
            None,
        )
        .await
        .unwrap();
        let entity = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title": "arbitrary widths"}),
            },
            None,
        )
        .await
        .unwrap();
        let transaction = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
        let other_workspace = yorishiro::models::tenancy::create_workspace(
            &transaction,
            fixture.tenant_id,
            "other-width-workspace",
            None,
            None,
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            other_workspace.id,
            serde_json::from_value(note_definition()).unwrap(),
            None,
            None,
        )
        .await
        .unwrap();
        let other = entity_entities::create(
            &ctx.db,
            other_workspace.id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title":"other workspace"}),
            },
            None,
        )
        .await
        .unwrap();
        let (other_provider, _other_server) = fixed_width_provider(11).await;
        entity_embeddings::reindex_workspace(&ctx.db, other_workspace.id, &other_provider)
            .await
            .unwrap();
        for width in [7, 13, 7] {
            let (provider, _server) = fixed_width_provider(width).await;
            let outcome =
                entity_embeddings::reindex_workspace(&ctx.db, fixture.workspace_id, &provider)
                    .await
                    .unwrap();
            assert!(outcome.failures.is_empty());
            assert_eq!(outcome.reindexed, 1);
            let workspace = workspace_workspaces::Entity::find_by_id(fixture.workspace_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(workspace.embedding_dimensions, Some(width as i32));
            use sea_orm::sea_query::{Alias, Expr, Func};
            use yorishiro::models::_entities::entity_embeddings as embeddings;
            let length = Func::cust(Alias::new(
                if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                    "length"
                } else {
                    "vector_dims"
                },
            ))
            .arg(Expr::col(embeddings::Column::Embedding));
            let stored = embeddings::Entity::find_by_id(entity.id)
                .select_only()
                .expr(length)
                .into_tuple::<i32>()
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            let expected = if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                width * std::mem::size_of::<f32>()
            } else {
                width
            };
            assert_eq!(stored, expected as i32);
            assert_eq!(
                embeddings::Entity::find()
                    .filter(embeddings::Column::EntityId.is_in([entity.id, other.id]))
                    .count(&ctx.db)
                    .await
                    .unwrap(),
                2
            );
            let other_length = Func::cust(Alias::new(
                if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                    "length"
                } else {
                    "vector_dims"
                },
            ))
            .arg(Expr::col(embeddings::Column::Embedding));
            let retained = embeddings::Entity::find_by_id(other.id)
                .select_only()
                .expr(other_length)
                .into_tuple::<i32>()
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                retained,
                if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                    11 * std::mem::size_of::<f32>() as i32
                } else {
                    11
                }
            );
            let hits = search::search_by_vector(
                &ctx.db,
                fixture.workspace_id,
                vec![1.0; width],
                "arbitrary",
                search::SearchQuery::default(),
            )
            .await
            .unwrap();
            assert_eq!(hits[0].entity.id, entity.id);
        }
    })
    .await;
}
