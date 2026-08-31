use async_trait::async_trait;
use loco_rs::testing::prelude::*;
use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
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

/// A workspace stamped `"unconfigured"` (created before #279's stamping fix, or with no embedding provider available at all) has no real record of what it was embedded with, so the model check must not compare against that string: refusing every write to a workspace this check cannot actually help would stop every prerelease deployment from writing anything at all.
#[tokio::test]
#[serial]
async fn sync_embedding_skips_the_model_check_for_an_unconfigured_stamp() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("unconfigured-stamp-test".into()),
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
            embedding_model: sea_orm::ActiveValue::Set(Some("unconfigured".into())),
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
                data: serde_json::json!({ "title": "gets an embedding despite the stamp" }),
            },
            None,
        )
        .await
        .expect("create entity");

        let provider = FixedModelProvider("intfloat/multilingual-e5-base");
        let result =
            sync::sync_embedding_for_record(&ctx.db, workspace.id, &entity, &provider).await;
        assert!(result.is_ok(), "result: {result:?}");

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
            Some(true),
            "an \"unconfigured\" stamp must not block a write"
        );

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
