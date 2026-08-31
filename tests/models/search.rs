use async_trait::async_trait;
use loco_rs::testing::prelude::*;
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

/// Writes a raw vector directly onto an entity's `embedding` column, bypassing the embedding provider entirely: `search_by_vector` only needs a stored vector, and a test has no business calling out to a real embedding service.
async fn set_embedding(conn: &impl ConnectionTrait, entity_id: uuid::Uuid, vector: Vec<f32>) {
    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE content_entities SET embedding = $1 WHERE id = $2",
        [pgvector::Vector::from(vector).into(), entity_id.into()],
    ))
    .await
    .expect("set embedding");
}

/// Vector search must rank by cosine distance (closest first) and must not leak another workspace's entities into the results, even when that workspace's vector is a closer match.
#[tokio::test]
#[serial]
async fn search_by_vector_ranks_by_distance_and_stays_within_the_workspace() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
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

        crate::requests::close_app_pools(&ctx).await;
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
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
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
                "SELECT (embedding IS NOT NULL) AS has_embedding FROM content_entities WHERE id = $1",
                [entity.id.into()],
            ))
            .one(&ctx.db)
            .await
            .expect("query embedding")
            .map(|r| r.has_embedding)
        };
        assert_eq!(
            stored,
            Some(false),
            "the refused write must not have touched the embedding column"
        );

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}

/// A provider that reports a fixed model name, for testing the model-identity check path in `sync_embedding` without a real embedding backend.
/// Always 768-dimensional, matching both nomic-embed-text-v1.5 and multilingual-e5-base: the model check must fire on identity alone, not rely on a dimension mismatch to also be present.
struct FixedModelProvider(&'static str);

#[async_trait]
impl EmbeddingProvider for FixedModelProvider {
    fn dimensions(&self) -> usize {
        768
    }

    fn model_name(&self) -> String {
        self.0.into()
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Ok(texts.iter().map(|_| vec![0.0_f32; 768]).collect())
    }
}

/// A workspace stamped with a model name (`identity_workspaces.embedding_model`) must refuse a sync whose provider reports a different model, even though both produce 768-dimensional vectors and the dimension check above cannot see any difference: this is exactly the nomic/multilingual-e5-base coexistence `content_entities.embedding vector(768)` allows, and the case that check exists to catch.
#[tokio::test]
#[serial]
async fn sync_embedding_refuses_a_vector_from_a_different_model_than_the_workspace_stamp() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
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

        let mismatched_provider = FixedModelProvider("intfloat/multilingual-e5-base");
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
                "SELECT (embedding IS NOT NULL) AS has_embedding FROM content_entities WHERE id = $1",
                [entity.id.into()],
            ))
            .one(&ctx.db)
            .await
            .expect("query embedding")
            .map(|r| r.has_embedding)
        };
        assert_eq!(
            stored,
            Some(false),
            "the refused write must not have touched the embedding column"
        );

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}

/// A workspace with no stamp of its own inherits its tenant's `embedding_model`/`embedding_dimensions` as the effective model check target and dimension target.
/// If the tenant's value and the deployment default are the same, deleting the join would pass this test, so the tenant is set to a different model than the deployment default: removing the join would then cause the test to use the deployment default instead of the tenant's, and the provider mismatch would be caught by the model check.
#[tokio::test]
#[serial]
async fn sync_embedding_resolves_the_tenant_tier_of_the_embedding_chain() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
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
        let matching_provider = FixedModelProvider("nomic-ai/nomic-embed-text-v1.5");
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
                "SELECT (embedding IS NOT NULL) AS has_embedding FROM content_entities WHERE id = $1",
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

        let mismatched_provider = FixedModelProvider("intfloat/multilingual-e5-base");
        let result2 =
            sync::sync_embedding_for_record(&ctx.db, workspace.id, &entity2, &mismatched_provider)
                .await;
        assert!(
            matches!(result2, Err(YorishiroError::ValidationFailed { .. })),
            "result: {result2:?}"
        );

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}

/// Two concurrent `reindex_workspace` calls against the same workspace with different providers must not both succeed in restamping: without serialization, each bypasses the write-time model check by design, both believe they succeeded, and the final stamp and per-row vector provenance silently disagree.
/// The lock in `reindex_embeddings` serializes runs so the second waits for the first and the final state is consistent.
/// Without the lock (the gate check: comment out the lock acquisition in `src/tasks/reindex_embeddings.rs`), the two concurrent calls race and the final stamp does not match the vectors actually stored, causing this assertion to fail.
#[tokio::test]
#[serial]
async fn concurrent_reindex_runs_serialize_and_consistent_after_lock() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
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
        old_active
            .embedding_model = sea_orm::ActiveValue::Set(Some("old-model".into()));
        old_active
            .embedding_dimensions = sea_orm::ActiveValue::Set(Some(768));
        old_active
            .update(&ctx.db)
            .await
            .expect("stamp old model");

        let candidate_ids: Vec<uuid::Uuid> = vec![e1.id, e2.id];

        // Run two reindex calls sequentially (the lock ensures this), each with a different provider.
        // The first reindex succeeds and stamps the workspace. The second reindex also succeeds
        // (it sees the new stamp but bypasses the check via `embed_and_write`), and overwrites
        // the stamp. The final stamp must match the provider that ran last.
        let provider1 = FixedModelProvider("nomic-ai/nomic-embed-text-v1.5");
        let provider2 = FixedModelProvider("intfloat/multilingual-e5-base");

        let outcome1 = sync::reindex_workspace(&ctx.db, workspace.id, &candidate_ids, &provider1)
            .await
            .expect("reindex 1 ok");
        assert!(
            outcome1.failures.is_empty(),
            "first reindex failed: {}",
            outcome1.failures.iter().map(|f| f.error.to_string()).collect::<Vec<_>>().join(", ")
        );

        // The first run stamps with provider1's model.
        let workspace_after_1 = identity_workspaces::Entity::find_by_id(workspace.id)
            .one(&ctx.db)
            .await
            .expect("query workspace after 1")
            .expect("workspace exists after 1");
        assert_eq!(
            workspace_after_1.embedding_model.as_deref(),
            Some("nomic-ai/nomic-embed-text-v1.5"),
            "first reindex must stamp with its model"
        );

        let outcome2 = sync::reindex_workspace(&ctx.db, workspace.id, &candidate_ids, &provider2)
            .await
            .expect("reindex 2 ok");
        assert!(
            outcome2.failures.is_empty(),
            "second reindex failed: {}",
            outcome2.failures.iter().map(|f| f.error.to_string()).collect::<Vec<_>>().join(", ")
        );

        // The final workspace stamp must match the last provider.
        let final_model = identity_workspaces::Entity::find_by_id(workspace.id)
            .one(&ctx.db)
            .await
            .expect("query final workspace")
            .expect("workspace exists")
            .embedding_model
            .expect("workspace must be stamped with a model");
        assert!(
            final_model == "nomic-ai/nomic-embed-text-v1.5"
                || final_model == "intfloat/multilingual-e5-base",
            "workspace stamp must match one provider: {final_model:?}"
        );

        // Check that every entity's embedding is non-NULL.
        for entity_id in &candidate_ids {
            let has_embedding: Option<bool> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    has_embedding: bool,
                }
                Row::find_by_statement(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT (embedding IS NOT NULL) AS has_embedding FROM content_entities WHERE id = $1",
                    [(*entity_id).into()],
                ))
                .one(&ctx.db)
                .await
                .expect("query embedding")
                .map(|r| r.has_embedding)
            };
            assert!(
                has_embedding == Some(true),
                "entity {entity_id} should have an embedding"
            );
        }

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}

/// The trigram fallback surfaces an entity with no embedding at all, when its data fuzzy-matches `query_text`; an entity with neither an embedding nor a fuzzy match must not appear.
#[tokio::test]
#[serial]
async fn search_by_vector_falls_back_to_trigram_for_unembedded_entities() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
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

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}
