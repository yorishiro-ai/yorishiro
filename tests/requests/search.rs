use super::boot_request;
use async_trait::async_trait;
use axum::http::StatusCode;
use serial_test::serial;
use std::sync::{Arc, Mutex};
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{api_keys, tenant_tenants, workspace_workspaces};
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenant_memberships::MembershipRole;
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::embedding::{EmbedKind, EmbeddingProvider};

struct Setup {
    read_key: String,
    tenant_id: uuid::Uuid,
    workspace_id: uuid::Uuid,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("search-req-test".into()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");
    let owner = yorishiro::models::user_users::create_user(
        &ctx.db,
        "owner@example.com",
        "hunter2-hunter2",
        None,
    )
    .await
    .expect("create owner");
    yorishiro::models::tenant_memberships::add_member(
        &ctx.db,
        tenant.id,
        owner.id,
        MembershipRole::Owner,
    )
    .await
    .expect("add owner");
    let read_key = api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Read,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue read key")
    .plaintext;
    Setup {
        read_key,
        tenant_id: tenant.id,
        workspace_id: workspace.id,
    }
}

struct RecordingProvider {
    kinds: Arc<Mutex<Vec<EmbedKind>>>,
}

#[async_trait]
impl EmbeddingProvider for RecordingProvider {
    fn dimensions(&self) -> usize {
        768
    }

    fn model_name(&self) -> String {
        "recording-provider".into()
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Ok(texts.iter().map(|_| vec![0.0; 768]).collect())
    }

    async fn embed_as(&self, kind: EmbedKind, _text: &str) -> Result<Vec<f32>, YorishiroError> {
        self.kinds.lock().unwrap().push(kind);
        Ok(vec![1.0; 768])
    }
}

#[tokio::test]
async fn search_embeds_the_query_synchronously_as_query_kind() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let Setup { read_key, .. } = setup(&ctx).await;
        let kinds = Arc::new(Mutex::new(Vec::new()));
        ctx.shared_store.insert(Arc::new(RecordingProvider {
            kinds: kinds.clone(),
        }) as Arc<dyn EmbeddingProvider>);

        let response = request
            .get("/api/search?query_text=hello")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;

        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "{}",
            response.text()
        );
        assert!(
            kinds.lock().unwrap().as_slice() == [EmbedKind::Query],
            "search must make exactly one query embedding call"
        );
    })
    .await;
}

/// Entity create, the real worker, then `GET /api/search` through the tenant-scoped transaction: the whole semantic path on whichever backend `DATABASE_URL` names.
/// The worker embeds as `Document` and the server embeds the query as `Query`, both through the same provider.
#[tokio::test]
async fn search_finds_an_entity_embedded_by_the_worker() {
    use loco_rs::bgworker::BackgroundWorker;
    use yorishiro::models::{entity_entities, schema_schemas};
    use yorishiro::workers::embedding_sync::{
        EmbeddingSyncArgs, EmbeddingSyncWorkerShared, WorkerClass,
    };

    boot_request::<App, _, _>(|request, ctx| async move {
        let Setup {
            read_key,
            tenant_id,
            workspace_id,
        } = setup(&ctx).await;
        let definition = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": { "note": { "fields": {
                "title": { "type": "string", "required": true, "x-embed": true }
            } } }
        }))
        .expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, definition, None, None)
            .await
            .expect("create schema");
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

        let kinds = Arc::new(Mutex::new(Vec::new()));
        ctx.shared_store.insert(Arc::new(RecordingProvider {
            kinds: kinds.clone(),
        }) as Arc<dyn EmbeddingProvider>);

        EmbeddingSyncWorkerShared::build(&ctx)
            .perform(EmbeddingSyncArgs {
                lifecycle_id: None,
                workspace_id,
                entity_id: entity.id,
                worker_class: WorkerClass::Shared,
            })
            .await
            .expect("worker embeds the entity");

        let response = request
            .get("/api/search?query_text=roadmap")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "{}",
            response.text()
        );
        let hits: serde_json::Value = response.json();
        assert_eq!(hits.as_array().map(Vec::len), Some(1), "hits: {hits}");
        assert_eq!(hits[0]["entity"]["id"], entity.id.to_string());
        assert!(
            hits[0]["distance"].is_number(),
            "the hit must come from the stored vector, not the text fallback: {hits}"
        );
        assert!(
            kinds.lock().unwrap().as_slice() == [EmbedKind::Document, EmbedKind::Query],
            "the worker embeds as Document, the server as Query"
        );
    })
    .await;
}

/// `YORISHIRO_EMBEDDING_PROVIDER=none` forces `build_embedding_provider` to return `UnconfiguredEmbeddingProvider` regardless of cached model files, so any actual search attempt surfaces as 502, not a panic or a silent empty result.
/// Search fails loudly and namedly rather than the boot process itself failing for every deployment that hasn't configured embeddings yet.
#[tokio::test]
#[serial(process_environment)]
async fn search_with_no_embedding_provider_configured_returns_502() {
    // Force unconfigured provider even if model files exist in cache: the test asserts on the
    // provider-missing path, not on the local provider succeeding.
    let guard = crate::EnvGuard::capture(&["YORISHIRO_EMBEDDING_PROVIDER"]);
    guard.set("YORISHIRO_EMBEDDING_PROVIDER", "none");

    boot_request::<App, _, _>(|request, ctx| async move {
        let Setup { read_key, .. } = setup(&ctx).await;

        let response = request
            .get("/api/search?query_text=hello")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(
            response.status_code(),
            502,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// The auth check (`Verified`) runs before the embedding call: no key at all must be rejected with 401, not 502.
/// `Read` is the lowest scope, so there's no "too-low scope" case to test against a read-gated endpoint beyond this.
#[tokio::test]
#[serial(process_environment)]
async fn search_requires_authentication() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/api/search?query_text=hello").await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    })
    .await;
}
