use std::sync::atomic::{AtomicUsize, Ordering};

use crate::requests::boot_request;
use async_trait::async_trait;
use sea_orm::FromQueryResult;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::{content_entities, content_schemas};
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

async fn insert_workspace(
    ctx: &loco_rs::app::AppContext,
    stamped_model: &str,
) -> identity_workspaces::Model {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("reindex-embeddings-test".into()),
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
        embedding_model: sea_orm::ActiveValue::Set(Some(stamped_model.into())),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");
    let def = serde_json::from_value(note_definition()).expect("parse definition");
    content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
        .await
        .expect("create schema");
    workspace
}

async fn insert_entity(
    ctx: &loco_rs::app::AppContext,
    workspace_id: uuid::Uuid,
    title: &str,
) -> content_entities::EntityRecord {
    content_entities::create(
        &ctx.db,
        workspace_id,
        content_entities::CreateEntityInput {
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
    #[derive(FromQueryResult)]
    struct Row {
        embedding_model: Option<String>,
    }
    Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT embedding_model FROM identity_workspaces WHERE id = $1",
        [workspace_id.into()],
    ))
    .one(&ctx.db)
    .await
    .expect("query stamp")
    .and_then(|r| r.embedding_model)
}

async fn embedding_is_set(ctx: &loco_rs::app::AppContext, entity_id: uuid::Uuid) -> bool {
    #[derive(FromQueryResult)]
    struct Row {
        has_embedding: bool,
    }
    Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT (embedding IS NOT NULL) AS has_embedding FROM content_entity_embeddings WHERE entity_id = $1",
        [entity_id.into()],
    ))
    .one(&ctx.db)
    .await
    .expect("query embedding")
    .map(|r| r.has_embedding)
    .unwrap_or(false)
}

/// A provider that succeeds its first `succeeds_before_failing` calls, then errors on every call after: enough to make a real reindex run produce a genuine partial result (some rows re-embedded, some not) rather than an all-or-nothing outcome that never exercises the restamp-ordering logic.
struct FlakyProvider {
    model_name: &'static str,
    calls: AtomicUsize,
    succeeds_before_failing: usize,
}

#[async_trait]
impl EmbeddingProvider for FlakyProvider {
    fn dimensions(&self) -> usize {
        768
    }

    fn model_name(&self) -> String {
        self.model_name.into()
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call >= self.succeeds_before_failing {
            return Err(YorishiroError::Internal(anyhow::anyhow!(
                "simulated provider outage"
            )));
        }
        Ok(texts.iter().map(|_| vec![0.0_f32; 768]).collect())
    }
}

/// A provider that always succeeds, for the happy-path restamp test.
struct WorkingProvider(&'static str);

#[async_trait]
impl EmbeddingProvider for WorkingProvider {
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

/// A provider that, on its first call, updates `target_entity_id`'s own data (bumping `updated_at`) before returning a vector, deterministically recreating the race `embed_and_write`'s `updated_at` guard exists for: `reindex_embedding_for_record` reads `record.updated_at` before this call runs, so the write that follows targets a now-stale snapshot and matches zero rows.
struct ConcurrentModificationProvider {
    model_name: &'static str,
    conn: sea_orm::DatabaseConnection,
    workspace_id: uuid::Uuid,
    target_entity_id: uuid::Uuid,
    triggered: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl EmbeddingProvider for ConcurrentModificationProvider {
    fn dimensions(&self) -> usize {
        768
    }

    fn model_name(&self) -> String {
        self.model_name.into()
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        if !self.triggered.swap(true, Ordering::SeqCst) {
            content_entities::update(
                &self.conn,
                self.workspace_id,
                self.target_entity_id,
                serde_json::json!({ "title": "modified concurrently with the reindex" }),
                None,
            )
            .await
            .expect("concurrent update");
        }
        Ok(texts.iter().map(|_| vec![0.0_f32; 768]).collect())
    }
}

/// A partial failure must leave the workspace's stamp completely unchanged, which is the load-bearing property `reindex_embeddings` exists to guarantee: restamping on a partial result would claim a model for rows that were never actually re-embedded with it, recreating the exact stamp/data mismatch the write-time model check in `sync.rs` exists to catch.
#[tokio::test]
#[serial]
async fn reindex_workspace_leaves_the_stamp_unchanged_on_partial_failure() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let workspace = insert_workspace(&ctx, "nomic-ai/nomic-embed-text-v1.5").await;
        let first = insert_entity(&ctx, workspace.id, "first entity").await;
        let second = insert_entity(&ctx, workspace.id, "second entity").await;

        let provider = FlakyProvider {
            model_name: "intfloat/multilingual-e5-base",
            calls: AtomicUsize::new(0),
            succeeds_before_failing: 1,
        };
        let outcome =
            sync::reindex_workspace(&ctx.db, workspace.id, &[first.id, second.id], &provider)
                .await
                .expect("reindex_workspace runs even with per-entity failures");

        assert_eq!(outcome.total, 2);
        assert_eq!(
            outcome.reindexed, 1,
            "exactly one entity should have embedded before the simulated outage"
        );
        assert_eq!(outcome.failures.len(), 1);

        assert_eq!(
            stamped_model(&ctx, workspace.id).await,
            Some("nomic-ai/nomic-embed-text-v1.5".into()),
            "a partial failure must not touch the workspace's stamp"
        );
    })
    .await;
}

/// Full success must both write every row's vector and restamp the workspace to the new provider's identity, which is what lets a subsequent ordinary write pass the write-time model check instead of being refused forever.
#[tokio::test]
#[serial]
async fn reindex_workspace_restamps_only_after_every_entity_succeeds() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let workspace = insert_workspace(&ctx, "nomic-ai/nomic-embed-text-v1.5").await;
        let first = insert_entity(&ctx, workspace.id, "first entity").await;
        let second = insert_entity(&ctx, workspace.id, "second entity").await;

        let provider = WorkingProvider("intfloat/multilingual-e5-base");
        let outcome = sync::reindex_workspace(
            &ctx.db,
            workspace.id,
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
            stamped_model(&ctx, workspace.id).await,
            Some("intfloat/multilingual-e5-base".into()),
            "full success must restamp the workspace to the provider that actually wrote the vectors"
        );

    })
    .await;
}

/// An entity modified between `reindex_workspace`'s batch fetch and its write landing must be reported as a failure, not silently counted as reindexed, or the workspace would restamp while that row still holds a vector from before the concurrent modification: the exact stamp/data mismatch the write-time model check exists to catch, this time caused by the migration tool racing an ordinary write instead of a misconfigured deployment.
#[tokio::test]
#[serial]
async fn reindex_workspace_reports_a_concurrently_modified_entity_as_a_failure() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let workspace = insert_workspace(&ctx, "nomic-ai/nomic-embed-text-v1.5").await;
        let first = insert_entity(&ctx, workspace.id, "first entity").await;
        let second = insert_entity(&ctx, workspace.id, "second entity").await;

        let provider = ConcurrentModificationProvider {
            model_name: "intfloat/multilingual-e5-base",
            conn: ctx.db.clone(),
            workspace_id: workspace.id,
            target_entity_id: first.id,
            triggered: std::sync::atomic::AtomicBool::new(false),
        };
        let outcome =
            sync::reindex_workspace(&ctx.db, workspace.id, &[first.id, second.id], &provider)
                .await
                .expect("reindex_workspace runs even with a concurrent modification");

        assert_eq!(outcome.total, 2);
        assert_eq!(
            outcome.reindexed, 1,
            "only the entity that was not concurrently modified should count as reindexed"
        );
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].entity_id, first.id);

        assert_eq!(
            stamped_model(&ctx, workspace.id).await,
            Some("nomic-ai/nomic-embed-text-v1.5".into()),
            "a concurrently modified entity must block the restamp exactly like any other failure"
        );
    })
    .await;
}
