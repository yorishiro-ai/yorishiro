use super::boot_request;
use super::query_worker::{FailingProvider, KeywordProvider, boot_with_query_worker};
use axum::http::StatusCode;
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{api_keys, tenant_tenants, workspace_workspaces};
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::query_embedding_requests;
use yorishiro::models::tenant_memberships::MembershipRole;
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::{entity_embeddings, entity_entities, schema_schemas};
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

/// Creates the `note` schema and one embedded entity per title, embedding each with `provider` directly.
/// Document embedding through the queue is covered by the embedding worker's own tests, so these tests fix the document side and exercise only the query side.
async fn embedded_notes(
    ctx: &loco_rs::app::AppContext,
    setup: &Setup,
    provider: &dyn EmbeddingProvider,
    titles: &[&str],
) {
    let definition = serde_json::from_value(serde_json::json!({
        "name": "note",
        "entity_types": { "note": { "fields": {
            "title": { "type": "string", "required": true, "x-embed": true }
        } } }
    }))
    .expect("parse definition");
    schema_schemas::create_schema(
        &ctx.db,
        setup.tenant_id,
        setup.workspace_id,
        definition,
        None,
        None,
    )
    .await
    .expect("create schema");
    for title in titles {
        let record = entity_entities::create(
            &ctx.db,
            setup.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": title }),
            },
            None,
        )
        .await
        .expect("create entity");
        entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            setup.workspace_id,
            &record,
            provider,
        )
        .await
        .expect("embed entity");
    }
}

async fn rows_left(ctx: &loco_rs::app::AppContext) -> u64 {
    use sea_orm::{EntityTrait, PaginatorTrait};
    query_embedding_requests::Entity::find()
        .count(&ctx.db)
        .await
        .expect("count requests")
}

fn titles(hits: &serde_json::Value) -> Vec<String> {
    hits.as_array()
        .expect("search hits")
        .iter()
        .map(|hit| hit["entity"]["data"]["title"].as_str().unwrap().to_owned())
        .collect()
}

/// The whole semantic path with the model only in the worker: the server enqueues the query, a worker embeds it as `Query`, and the vector ranks the entities.
/// Two searches run at once, and each must get the vector for its own text.
#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn search_asks_a_worker_to_embed_the_query_and_ranks_by_the_result() {
    let provider = Arc::new(KeywordProvider::new(768));
    let kinds = provider.kinds.clone();
    boot_with_query_worker(provider.clone(), |request, ctx| async move {
        let setup = setup(&ctx).await;
        embedded_notes(
            &ctx,
            &setup,
            provider.as_ref(),
            &["alpha plan", "beta plan", "gamma plan"],
        )
        .await;
        let documents = kinds.lock().unwrap().len();
        assert_eq!(documents, 3, "setup embedded three documents");

        let search = |query: &'static str| {
            let request = &request;
            let key = setup.read_key.clone();
            async move {
                request
                    .get(&format!("/api/search?query_text={query}&limit=3"))
                    .add_header("Authorization", format!("Bearer {key}"))
                    .await
            }
        };
        let (beta, alpha) = tokio::join!(search("beta"), search("alpha"));
        for (response, expected) in [(beta, "beta plan"), (alpha, "alpha plan")] {
            assert_eq!(
                response.status_code(),
                StatusCode::OK,
                "{}",
                response.text()
            );
            let hits: serde_json::Value = response.json();
            assert_eq!(titles(&hits)[0], expected, "nearest entity first: {hits}");
            assert!(
                hits[0]["distance"].as_f64().unwrap() < 1.0e-6,
                "a shared keyword is at distance zero: {hits}"
            );
        }
        let queries = kinds.lock().unwrap()[documents..].to_vec();
        assert!(
            queries.len() == 2 && queries.iter().all(|kind| *kind == EmbedKind::Query),
            "the worker embeds each query once, as a query"
        );
        assert_eq!(rows_left(&ctx).await, 0, "consumed results are deleted");
    })
    .await;
}

/// With no worker running the search must fail, loudly and boundedly: a lexical fallback would return results the caller did not ask for.
#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn search_without_a_worker_times_out_as_service_unavailable_without_a_fallback() {
    let guard = crate::EnvGuard::capture(&[
        "YORISHIRO_QUERY_EMBEDDING_TIMEOUT_MS",
        "YORISHIRO_QUERY_EMBEDDING_POLL_INTERVAL_MS",
    ]);
    guard.set("YORISHIRO_QUERY_EMBEDDING_TIMEOUT_MS", "300");
    guard.set("YORISHIRO_QUERY_EMBEDDING_POLL_INTERVAL_MS", "50");
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;
        // An entity whose text matches the query literally, so a lexical fallback would find it.
        embedded_notes(&ctx, &setup, &KeywordProvider::new(768), &["alpha plan"]).await;

        let started = std::time::Instant::now();
        let response = request
            .get("/api/search?query_text=alpha")
            .add_header("Authorization", format!("Bearer {}", setup.read_key))
            .await;
        let waited = started.elapsed();

        assert_eq!(
            response.status_code(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["error"]["code"], "provider_busy", "{body}");
        assert!(body["error"]["retry_after_seconds"].as_u64().is_some());
        assert!(
            waited >= Duration::from_millis(300) && waited < Duration::from_secs(10),
            "the wait is bounded by the configured timeout: {waited:?}"
        );
        assert_eq!(rows_left(&ctx).await, 0, "the expired request is consumed");
    })
    .await;
}

/// A worker that answers after the search stopped waiting must not resurrect the request or fail.
/// The queue polls once a second, so the timeout is long enough for the worker to claim the job and the provider slower than the timeout.
#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn a_result_that_arrives_after_the_timeout_is_discarded() {
    let guard = crate::EnvGuard::capture(&[
        "YORISHIRO_QUERY_EMBEDDING_TIMEOUT_MS",
        "YORISHIRO_QUERY_EMBEDDING_POLL_INTERVAL_MS",
    ]);
    guard.set("YORISHIRO_QUERY_EMBEDDING_TIMEOUT_MS", "1300");
    guard.set("YORISHIRO_QUERY_EMBEDDING_POLL_INTERVAL_MS", "100");
    let mut slow = KeywordProvider::new(768);
    slow.delay = Duration::from_secs(2);
    let kinds = slow.kinds.clone();
    boot_with_query_worker(Arc::new(slow), |request, ctx| async move {
        let setup = setup(&ctx).await;
        let response = request
            .get("/api/search?query_text=alpha")
            .add_header("Authorization", format!("Bearer {}", setup.read_key))
            .await;
        assert_eq!(response.status_code(), StatusCode::SERVICE_UNAVAILABLE);

        tokio::time::timeout(Duration::from_secs(10), async {
            while kinds.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("the worker eventually embeds the late query");
        tokio::time::sleep(Duration::from_millis(2300)).await;
        assert_eq!(
            rows_left(&ctx).await,
            0,
            "the late result neither revives nor leaks the request"
        );
    })
    .await;
}

/// A worker that cannot embed records why, and the search reports it as an unavailable service rather than a bad request.
#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn a_worker_failure_reaches_the_caller_as_service_unavailable() {
    let provider = Arc::new(FailingProvider(|| YorishiroError::ProviderUnreachable {
        url: "http://embedding.invalid".into(),
        message: "connection refused".into(),
    }));
    boot_with_query_worker(provider, |request, ctx| async move {
        let setup = setup(&ctx).await;
        let response = request
            .get("/api/search?query_text=alpha")
            .add_header("Authorization", format!("Bearer {}", setup.read_key))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["error"]["code"], "backend_unavailable", "{body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("connection refused"),
            "the worker's diagnostic is kept: {body}"
        );
        assert_eq!(rows_left(&ctx).await, 0);
    })
    .await;
}

/// A model whose width the workspace's vectors do not have is refused by the worker, against the same registry the writes use.
#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn a_width_the_workspace_does_not_hold_is_refused_by_the_worker() {
    boot_with_query_worker(
        Arc::new(KeywordProvider::new(1024)),
        |request, ctx| async move {
            let setup = setup(&ctx).await;
            // The workspace already holds 768-wide vectors.
            embedded_notes(&ctx, &setup, &KeywordProvider::new(768), &["alpha plan"]).await;

            let response = request
                .get("/api/search?query_text=alpha")
                .add_header("Authorization", format!("Bearer {}", setup.read_key))
                .await;
            assert_eq!(
                response.status_code(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{}",
                response.text()
            );
            let message = response.text();
            assert!(
                message.contains("768") && message.contains("1024"),
                "the diagnostic names both widths: {message}"
            );
        },
    )
    .await;
}

/// `YORISHIRO_EMBEDDING_PROVIDER=none` on the worker is an accurate, immediate answer, not a timeout.
#[tokio::test]
#[serial(queue_postgres)]
#[serial(process_environment)]
async fn a_disabled_embedding_provider_is_reported_by_the_worker() {
    let settings: yorishiro::data::settings::Settings = serde_json::from_value(serde_json::json!({
        "max_tenants": 1,
        "embedding": {
            "provider": "none", "dimensions": 768, "base_url": null, "api_key": "",
            "model": null, "send_dimensions_param": false,
            "local_model": "multilingual-e5-base", "local_max_sequence_length": 512
        },
        "rate_limit": {
            "auth_max_requests": 10, "auth_window_seconds": 60, "search_tokens_per_minute": 100000
        },
        "db_load_guard": { "threshold": 0, "sustain_seconds": 30, "poll_seconds": 5 }
    }))
    .expect("settings");
    let settings_provider = yorishiro::services::embedding::build_embedding_provider(&settings)
        .await
        .expect("provider");
    boot_with_query_worker(settings_provider, |request, ctx| async move {
        let setup = setup(&ctx).await;
        let response = request
            .get("/api/search?query_text=alpha")
            .add_header("Authorization", format!("Bearer {}", setup.read_key))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            response.text()
        );
        assert!(
            response
                .text()
                .contains("no embedding provider is configured")
        );
    })
    .await;
}

/// The auth check (`Verified`) runs before the query is queued: no key at all must be rejected with 401, not 503.
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
