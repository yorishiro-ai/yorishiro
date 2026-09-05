use crate::requests::boot_request;
use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, FromQueryResult, Statement};
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::{content_entities, content_schemas, search};
use yorishiro::services::embedding::EmbeddingProvider;
use yorishiro::services::embedding::sync;

fn note_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "note",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true, "x-embed": true } } }
        }
    })
}

/// Writes a raw vector directly into `content_entity_embeddings`, bypassing the embedding provider entirely: `search_by_vector` only needs a stored vector, and a test has no business calling out to a real embedding service.
async fn set_embedding(conn: &impl ConnectionTrait, entity_id: uuid::Uuid, vector: Vec<f32>) {
    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO content_entity_embeddings (entity_id, embedding) VALUES ($1, $2)\
         ON CONFLICT(entity_id) DO UPDATE SET embedding = $2",
        [
            entity_id.into(),
            sea_orm::entity::prelude::PgVector::from(vector).into(),
        ],
    ))
    .await
    .expect("set embedding");
}

/// Vector search must rank by cosine distance (closest first) and must not leak another workspace's entities into the results, even when that workspace's vector is a closer match.
#[tokio::test]
#[serial]
async fn search_by_vector_ranks_by_distance_and_stays_within_the_workspace() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("search-test".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");

        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");

        let other_workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("other".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let other_workspace = sea_orm::ActiveModelTrait::insert(other_workspace, &ctx.db)
            .await
            .expect("insert other workspace");

        let def = serde_json::from_value(note_definition()).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");
        let other_def = serde_json::from_value(note_definition()).expect("parse definition");
        content_schemas::create_schema(
            &ctx.db,
            tenant.id,
            other_workspace.id,
            other_def,
            None,
            None,
        )
        .await
        .expect("create schema in other workspace");

        let close = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "close match" }),
            },
            None,
        )
        .await
        .expect("create close entity");
        let far = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "far match" }),
            },
            None,
        )
        .await
        .expect("create far entity");
        let other_workspace_entity = content_entities::create(
            &ctx.db,
            other_workspace.id,
            content_entities::CreateEntityInput {
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

        let close_vector = unit_vector(0);
        let far_vector = unit_vector(1);
        let query_vector = close_vector.clone();
        set_embedding(&ctx.db, close.id, close_vector.clone()).await;
        set_embedding(&ctx.db, far.id, far_vector).await;
        // An exact match, but in a workspace the query never asks about: must never surface.
        set_embedding(&ctx.db, other_workspace_entity.id, close_vector).await;

        let hits = search::search_by_vector(
            &ctx.db,
            workspace.id,
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
            hits.iter().all(|h| h.entity.workspace_id == workspace.id),
            "no cross-workspace leakage: {hits:?}"
        );
    })
    .await;
}

/// A provider that always returns a fixed-width vector of zeros, for testing the dimension-check path in `sync_embedding` without a real embedding backend.
struct FixedWidthProvider(usize);

#[async_trait]
impl EmbeddingProvider for FixedWidthProvider {
    fn dimensions(&self) -> usize {
        self.0
    }

    fn model_name(&self) -> String {
        "fixed-width".into()
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Ok(texts.iter().map(|_| vec![0.0_f32; self.0]).collect())
    }
}

/// A workspace stamped with a dimension count (`identity_workspaces.embedding_dimensions`) must refuse a sync whose provider produces a different width, rather than writing a vector that would silently break every future search over that workspace with a dimension-mismatch error naming neither the entity nor the write that caused it.
#[tokio::test]
#[serial]
async fn sync_embedding_refuses_a_vector_that_does_not_match_the_workspace_stamp() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("dimension-mismatch-test".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");
        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            // Stamped at 768, matching this deployment's migrated HNSW index; the provider below deliberately produces 1024, the actual measured width of text-embedding-qwen3-embedding-0.6b, the other model LM Studio serves.
            embedding_dimensions: sea_orm::ActiveValue::Set(Some(768)),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");

        let entity = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "will not get an embedding" }),
            },
            None,
        )
        .await
        .expect("create entity");

        let mismatched_provider = FixedWidthProvider(1024);
        let result =
            sync::sync_embedding_for_record(&ctx.db, workspace.id, &entity, &mismatched_provider)
                .await;

        assert!(
            matches!(result, Err(YorishiroError::ValidationFailed { .. })),
            "result: {result:?}"
        );

        let stored: Option<bool> = {
            #[derive(sea_orm::FromQueryResult)]
            struct Row {
                has_embedding: bool,
            }
            Row::find_by_statement(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT (embedding IS NOT NULL) AS has_embedding FROM content_entity_embeddings WHERE entity_id = $1",
                [entity.id.into()],
            ))
            .one(&ctx.db)
            .await
            .expect("query embedding")
            .map(|r| r.has_embedding)
        };
        assert!(
            stored != Some(true),
            "the refused write must not have created an embedding row: {stored:?}"
        );

    })
    .await;
}

/// A provider that reports a fixed model name, for testing the model-identity check path in `sync_embedding` without a real embedding backend.
/// Always 768-dimensional, matching both nomic-embed-text-v1.5 and multilingual-e5-base: the model check must fire on identity alone, not rely on a dimension mismatch to also be present.
#[derive(Clone, Debug)]
struct FixedModelProvider(
    &'static str,
    &'static [u64],
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
);

impl FixedModelProvider {
    /// Returns a 768-dimensional vector that encodes the model name as a seed, so the stored vector proves which model last wrote it.
    fn vector(&self) -> Vec<f32> {
        let mut v = vec![0.0_f32; 768];
        let bytes = self.0.as_bytes();
        for (i, b) in bytes.iter().enumerate().take(768) {
            v[i] = *b as f32 / 255.0;
        }
        v
    }
}

#[async_trait]
impl EmbeddingProvider for FixedModelProvider {
    fn dimensions(&self) -> usize {
        768
    }

    fn model_name(&self) -> String {
        self.0.into()
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        // A per-call delay schedule, so two concurrent reindex runs can overtake each other
        // between rows. Symmetric delays cannot: whichever run starts a beat later stays a beat
        // later on every row, so it writes last everywhere and restamps last, and the stamp
        // agrees with the rows whether or not a lock is held. Only a crossover (run A writes
        // entity 1, run B then completes entirely, run A finally writes entity 2 and restamps)
        // produces the stamp/provenance disagreement this test exists to catch.
        let call = self.2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let delay = self.1.get(call).copied().unwrap_or(0);
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        Ok(texts.iter().map(|_| self.vector()).collect())
    }
}

/// A workspace stamped with a model name (`identity_workspaces.embedding_model`) must refuse a sync whose provider reports a different model, even though both produce 768-dimensional vectors and the dimension check above cannot see any difference: this is exactly the nomic/multilingual-e5-base coexistence `content_entities.embedding vector(768)` allows, and the case that check exists to catch.
#[tokio::test]
#[serial]
async fn sync_embedding_refuses_a_vector_from_a_different_model_than_the_workspace_stamp() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("model-mismatch-test".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");
        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            embedding_dimensions: sea_orm::ActiveValue::Set(Some(768)),
            embedding_model: sea_orm::ActiveValue::Set(Some(
                "nomic-ai/nomic-embed-text-v1.5".into(),
            )),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");

        let entity = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "will not get an embedding" }),
            },
            None,
        )
        .await
        .expect("create entity");

        let mismatched_provider = FixedModelProvider(
            "intfloat/multilingual-e5-base",
            &[],
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        let result =
            sync::sync_embedding_for_record(&ctx.db, workspace.id, &entity, &mismatched_provider)
                .await;

        assert!(
            matches!(result, Err(YorishiroError::ValidationFailed { .. })),
            "result: {result:?}"
        );

        let stored: Option<bool> = {
            #[derive(sea_orm::FromQueryResult)]
            struct Row {
                has_embedding: bool,
            }
            Row::find_by_statement(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT (embedding IS NOT NULL) AS has_embedding FROM content_entity_embeddings WHERE entity_id = $1",
                [entity.id.into()],
            ))
            .one(&ctx.db)
            .await
            .expect("query embedding")
            .map(|r| r.has_embedding)
        };
        assert!(
            stored != Some(true),
            "the refused write must not have created an embedding row: {stored:?}"
        );

    })
    .await;
}

/// A workspace with no stamp of its own inherits its tenant's `embedding_model`/`embedding_dimensions` as the effective model check target and dimension target.
/// If the tenant's value and the deployment default are the same, deleting the join would pass this test, so the tenant is set to a different model than the deployment default: removing the join would then cause the test to use the deployment default instead of the tenant's, and the provider mismatch would be caught by the model check.
#[tokio::test]
#[serial]
async fn sync_embedding_resolves_the_tenant_tier_of_the_embedding_chain() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("tenant-tier-test".into()),
            embedding_model: sea_orm::ActiveValue::Set(Some(
                "nomic-ai/nomic-embed-text-v1.5".into(),
            )),
            embedding_dimensions: sea_orm::ActiveValue::Set(Some(768)),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");
        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");

        let entity = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "resolves from tenant tier" }),
            },
            None,
        )
        .await
        .expect("create entity");

        // The tenant is stamped with nomic-embed-text-v1.5, so the effective model must be that.
        // Using the same model provider should succeed.
        let matching_provider = FixedModelProvider(
            "nomic-ai/nomic-embed-text-v1.5",
            &[],
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        let result =
            sync::sync_embedding_for_record(&ctx.db, workspace.id, &entity, &matching_provider)
                .await;
        assert!(result.is_ok(), "result: {result:?}");

        // Verify the embedding was written.
        let stored: Option<bool> = {
            #[derive(sea_orm::FromQueryResult)]
            struct Row {
                has_embedding: bool,
            }
            Row::find_by_statement(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT (embedding IS NOT NULL) AS has_embedding FROM content_entity_embeddings WHERE entity_id = $1",
                [entity.id.into()],
            ))
            .one(&ctx.db)
            .await
            .expect("query embedding")
            .map(|r| r.has_embedding)
        };
        assert_eq!(stored, Some(true), "embedding must have been written");

        // A different model should be refused by the inherited model check.
        let entity2 = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "wrong model, should be refused" }),
            },
            None,
        )
        .await
        .expect("create second entity");

        let mismatched_provider = FixedModelProvider(
            "intfloat/multilingual-e5-base",
            &[],
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        let result2 =
            sync::sync_embedding_for_record(&ctx.db, workspace.id, &entity2, &mismatched_provider)
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
#[serial]
async fn sync_embedding_resolves_the_tenant_dimension_tier() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("tenant-dimension-tier-test".into()),
            embedding_model: sea_orm::ActiveValue::Set(Some(
                "nomic-ai/nomic-embed-text-v1.5".into(),
            )),
            embedding_dimensions: sea_orm::ActiveValue::Set(Some(1024)),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");
        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");

        let entity = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "wrong dimension from tenant" }),
            },
            None,
        )
        .await
        .expect("create entity");

        // Provider is 768-dimensional, not the workspace's 1024: the dimension check fires
        // first and rejects the write before the model check is reached.
        let provider = FixedWidthProvider(768);
        let result =
            sync::sync_embedding_for_record(&ctx.db, workspace.id, &entity, &provider).await;

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
/// Without the lock (the gate check: comment out the lock acquisition in
/// `src/tasks/reindex_embeddings.rs`), the two concurrent calls race and the final stamp does
/// not match the vectors actually stored, causing this assertion to fail.
#[tokio::test]
#[serial]
async fn concurrent_reindex_runs_serialize_and_consistent_after_lock() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("concurrent-reindex-test".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");
        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");

        // Create entities to reindex.
        let e1 = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity one" }),
            },
            None,
        )
        .await
        .expect("create entity 1");
        let e2 = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity two" }),
            },
            None,
        )
        .await
        .expect("create entity 2");

        // Pre-stamp with an old model to simulate a workspace that needs reindexing.
        let mut old_active = identity_workspaces::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(workspace.id),
            ..Default::default()
        };
        old_active.embedding_model = sea_orm::ActiveValue::Set(Some("old-model".into()));
        old_active.embedding_dimensions = sea_orm::ActiveValue::Set(Some(768));
        old_active.update(&ctx.db).await.expect("stamp old model");

        let candidate_ids: Vec<uuid::Uuid> = vec![e1.id, e2.id];

        // Crossover schedule: provider1 writes entity 1 immediately then stalls, while
        // provider2 runs both rows and restamps inside that stall. Unlocked, entity 1 ends up
        // holding provider2's vector while provider1 restamps last, so the stamp and the rows
        // disagree. Held under the lock, one run simply finishes before the other starts.
        let provider1 = FixedModelProvider(
            "nomic-ai/nomic-embed-text-v1.5",
            &[0, 400],
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        let provider2 = FixedModelProvider(
            "intfloat/multilingual-e5-base",
            &[50, 0],
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );

        // Grab the tenant pool so we can call reindex_workspace_with_lock under it.
        let pool = ctx
            .shared_store
            .get::<yorishiro::db::DbHandle>()
            .expect("DbHandle is configured")
            .tenant
            .pool()
            .clone();

        let db_conn = ctx.db.clone();

        // Race two reindex calls against the same workspace using
        // tokio::sync::Barrier (async). Both tasks reach the barrier concurrently;
        // the first to resume acquires the lock, the second waits until the first
        // drops its guard. This tests lock serialization rather than sequential calls.
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let barrier1 = barrier.clone();
        let provider1_clone = provider1.clone();
        let pool1 = pool.clone();
        let cids1 = candidate_ids.clone();
        let db_conn1 = db_conn.clone();
        let h1 = tokio::spawn(async move {
            barrier1.wait().await;
            yorishiro::db::reindex_workspace_with_lock(
                pool1,
                workspace.id,
                &db_conn1,
                &cids1,
                &provider1_clone,
            )
            .await
            .expect("reindex 1 join")
        });
        let db_conn2 = db_conn.clone();
        let barrier2 = barrier.clone();
        let provider2_clone = provider2.clone();
        let pool2 = pool.clone();
        let cids2 = candidate_ids.clone();
        let h2 = tokio::spawn(async move {
            barrier2.wait().await;
            yorishiro::db::reindex_workspace_with_lock(
                pool2,
                workspace.id,
                &db_conn2,
                &cids2,
                &provider2_clone,
            )
            .await
            .expect("reindex 2 join")
        });

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
        let final_model = identity_workspaces::Entity::find_by_id(workspace.id)
            .one(&ctx.db)
            .await
            .expect("query final workspace")
            .expect("workspace exists")
            .embedding_model
            .expect("workspace must be stamped with a model");

        let winner = if final_model == "nomic-ai/nomic-embed-text-v1.5" {
            &provider1
        } else if final_model == "intfloat/multilingual-e5-base" {
            &provider2
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
                Row::find_by_statement(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT embedding FROM content_entity_embeddings WHERE entity_id = $1",
                    [(*entity_id).into()],
                ))
                .one(&ctx.db)
                .await
                .expect("query embedding")
                .and_then(|r| r.embedding)
            };
            let expected = winner.vector();
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
#[serial]
async fn reindex_overwrites_existing_entity_embeddings() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("reindex-overwrite-test".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");
        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");

        // Create entities.
        let e1 = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity one" }),
            },
            None,
        )
        .await
        .expect("create entity 1");
        let e2 = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity two" }),
            },
            None,
        )
        .await
        .expect("create entity 2");

        // Embed with the old model provider.
        let old_provider = FixedModelProvider(
            "old-model-a",
            &[],
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        for entity in [&e1, &e2] {
            let entity_record = content_entities::Entity::find_by_id(entity.id)
                .one(&ctx.db)
                .await
                .expect("find entity")
                .expect("entity exists");
            sync::sync_embedding_for_record(
                &ctx.db,
                workspace.id,
                &entity_record.into(),
                &old_provider,
            )
            .await
            .expect("embed with old model");
        }

        // Verify the old vectors are stored.
        let old_v1 = old_provider.vector();
        for entity_id in [e1.id, e2.id] {
            let stored: Option<sea_orm::entity::prelude::PgVector> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    embedding: Option<sea_orm::entity::prelude::PgVector>,
                }
                Row::find_by_statement(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT embedding FROM content_entity_embeddings WHERE entity_id = $1",
                    [entity_id.into()],
                ))
                .one(&ctx.db)
                .await
                .expect("query embedding")
                .and_then(|r| r.embedding)
            };
            assert_eq!(
                stored.as_ref().map(|v| v.as_slice()),
                Some(old_v1.as_slice()),
                "initial embedding must match old model {old_provider:?}"
            );
        }

        // Stamp the workspace with the old model so reindex considers it "in sync" — but
        // reindex's entire purpose is to re-embed regardless, so this stamp is only there
        // to make the test realistic.
        let mut old_active = identity_workspaces::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(workspace.id),
            ..Default::default()
        };
        old_active.embedding_model = sea_orm::ActiveValue::Set(Some("old-model-a".into()));
        old_active.embedding_dimensions = sea_orm::ActiveValue::Set(Some(768));
        old_active.update(&ctx.db).await.expect("stamp old model");

        // Reindex with the new model provider.
        let new_provider = FixedModelProvider(
            "new-model-b",
            &[],
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        let candidate_ids = vec![e1.id, e2.id];
        let outcome = yorishiro::services::embedding::sync::reindex_workspace(
            &ctx.db,
            workspace.id,
            &candidate_ids,
            &new_provider,
        )
        .await
        .expect("reindex");

        if !outcome.failures.is_empty() {
            panic!("reindex had {} failure(s)", outcome.failures.len());
        }
        assert_eq!(outcome.reindexed, 2, "both entities must be reindexed");

        // Stamp and vectors must now agree on the new model.
        let final_model = identity_workspaces::Entity::find_by_id(workspace.id)
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

        let new_v = new_provider.vector();

        // Per-row: assert_ne against old model (the row was NOT left untouched) and
        // assert_eq against new model (the new embedding was actually written).
        // Both halves are required: assert_ne alone passes on a zeroed or corrupted row,
        // assert_eq alone passes if the row already held the new value.
        for entity_id in [e1.id, e2.id] {
            let stored: Option<sea_orm::entity::prelude::PgVector> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    embedding: Option<sea_orm::entity::prelude::PgVector>,
                }
                Row::find_by_statement(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT embedding FROM content_entity_embeddings WHERE entity_id = $1",
                    [entity_id.into()],
                ))
                .one(&ctx.db)
                .await
                .expect("query embedding")
                .and_then(|r| r.embedding)
            };
            let old_vec = old_provider.vector();
            assert_ne!(
                stored.as_ref().map(|v| v.as_slice()),
                Some(old_vec.as_slice()),
                "entity {entity_id} embedding must NOT match old model {final_model:?}"
            );
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
#[serial]
async fn search_by_vector_falls_back_to_trigram_for_unembedded_entities() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("trigram-test".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");
        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");

        let matching = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "quarterly roadmap review" }),
            },
            None,
        )
        .await
        .expect("create matching entity");
        content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
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
            workspace.id,
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

/// The FTS5 fallback on SQLite surfaces an entity whose `data` fuzzy-matches `query_text` via the
/// FTS5 virtual table, when the entity has no embedding; an entity with neither an embedding nor
/// an FTS5 match must not appear.
///
/// On SQLite, `content_entities` has no `embedding` column, so the search function's trigram half
/// is replaced by an FTS5 MATCH query against the `fts_content_entities` virtual table created in
/// the migration. This test boots against a SQLite file database to confirm the FTS5 path works
/// end to end, including schema creation and entity insertion (which triggers FTS5 auto-sync).
#[tokio::test]
#[serial]
async fn search_by_vector_falls_back_to_fts5_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }

    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir
        .path()
        .join(format!("yorishiro_test_{}.sqlite3", uuid::Uuid::new_v4()));
    let db_path = db_path.to_str().expect("valid utf-8 path").to_string();
    crate::requests::boot_request_sqlite::<App, _, _>(
        db_path.clone(),
        |_request, ctx| async move {
            // Seed tenant/workspace with hex-string UUIDs so FK constraints match
            // the hex-string UUIDs written by `create_schema_sqlite` / `create_sqlite`.
            use sea_orm::Statement;
            let tenant_id = uuid::Uuid::now_v7();
            let workspace_id = uuid::Uuid::now_v7();
            ctx.db
                .execute_raw(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Sqlite,
                    "INSERT INTO identity_tenants (id, name) VALUES (?1, 'fts5-test')",
                    [sea_orm::Value::String(Some(tenant_id.to_string()))],
                ))
                .await
                .expect("insert tenant");
            ctx.db
                .execute_raw(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Sqlite,
                    "INSERT INTO identity_workspaces (id, tenant_id, name, status, max_entities) \
                     VALUES (?1, ?2, 'main', 'active', NULL)",
                    [
                        sea_orm::Value::String(Some(workspace_id.to_string())),
                        sea_orm::Value::String(Some(tenant_id.to_string())),
                    ],
                ))
                .await
                .expect("insert workspace");

            let def = serde_json::from_value(note_definition()).expect("parse definition");
            content_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
                .await
                .expect("create schema");

            // Create an entity whose title contains the search phrase.
            let matching = content_entities::create(
                &ctx.db,
                workspace_id,
                content_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: serde_json::json!({ "title": "quarterly roadmap review" }),
                },
                None,
            )
            .await
            .expect("create matching entity");

            // Create an entity whose title does not match.
            content_entities::create(
                &ctx.db,
                workspace_id,
                content_entities::CreateEntityInput {
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
            assert!(hits[0].distance.is_none(), "fts5-only hit has no distance");

            // Test the FTS5 UPDATE trigger: modify the entity's data so the old search
            // phrase no longer matches, then confirm a search for the new phrase finds it.
            use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
            use yorishiro::models::_entities::content_entities::Column;

            // Fetch using raw SQL (Entity::find_by_id selects embedding which doesn't
            // exist on SQLite). EntityRecord is a FromQueryResult type with the same
            // columns but no embedding field.
            use yorishiro::models::content_entities::EntityRecord;
            let rec = EntityRecord::find_by_statement(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT id, workspace_id, schema_id, schema_version, entity_type, data, \
                 created_at, updated_at, created_by, updated_by \
                 FROM content_entities WHERE id = ?",
                [matching.id.into()],
            ))
            .one(&ctx.db)
            .await
            .expect("fetch entity for update")
            .expect("entity exists");

            // Build an ActiveModel with the fetched fields and update.
            // On SQLite, `ActiveModelTrait::update` tries to decode the return value
            // as a `content_entities::Model` which includes `embedding` — doesn't exist
            // on SQLite. Use `update_without_returning` (same pattern as the production
            // `update_and_fetch` function).
            let active = content_entities::ActiveModel {
                id: sea_orm::ActiveValue::Set(rec.id),
                workspace_id: sea_orm::ActiveValue::Set(rec.workspace_id),
                schema_id: sea_orm::ActiveValue::Set(rec.schema_id),
                schema_version: sea_orm::ActiveValue::Set(rec.schema_version),
                entity_type: sea_orm::ActiveValue::Set(rec.entity_type),
                data: sea_orm::ActiveValue::Set(serde_json::json!({
                    "title": "quarterly board meeting notes"
                })),
                created_at: sea_orm::ActiveValue::Set(rec.created_at.into()),
                updated_at: sea_orm::ActiveValue::NotSet, // before_save stamps this
                created_by: sea_orm::ActiveValue::Set(rec.created_by),
                updated_by: sea_orm::ActiveValue::Set(rec.updated_by),
            };
            active
                .update_without_returning(&ctx.db)
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

            // Test the FTS5 DELETE trigger: delete the entity and verify the FTS5 side is actually
            // cleaned up (a bare search-by-vector assertion would be true even if the FTS5 row
            // lingered, because the join against content_entities would already find no match).
            content_entities::Entity::delete_many()
                .filter(Column::Id.eq(matching.id))
                .exec(&ctx.db)
                .await
                .expect("delete entity");

            let fts_count: Option<i64> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    cnt: i64,
                }
                Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Sqlite,
                    "SELECT COUNT(*) AS cnt FROM fts_content_entities WHERE entity_id = ?",
                    [matching.id.into()],
                ))
                .one(&ctx.db)
                .await
                .expect("fts count")
                .map(|r| r.cnt)
            };
            assert_eq!(
                fts_count,
                Some(0),
                "fts_content_entities must not contain the deleted row after trigger"
            );
        },
    )
    .await;
}
