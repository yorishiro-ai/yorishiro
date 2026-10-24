use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant};
use super::query_worker::{boot_with_query_worker, boot_with_query_worker_and_drain};
use crate::http_stub::{keyword_provider, unavailable_provider};
use axum::http::StatusCode;
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::query_embedding_requests;
use yorishiro::models::{entity_embeddings, entity_entities, schema_schemas};
use yorishiro::services::embedding::EmbeddingProvider;

/// Creates the `note` schema and one embedded entity per title, embedding each with `provider` directly.
/// Document embedding through the queue is covered by the embedding worker's own tests, so these tests fix the document side and exercise only the query side.
async fn embedded_notes(
    ctx: &loco_rs::app::AppContext,
    setup: &TestTenant,
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

async fn rows_left(ctx: &loco_rs::app::AppContext, workspace_id: uuid::Uuid) -> u64 {
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    query_embedding_requests::Entity::find()
        .filter(query_embedding_requests::Column::WorkspaceId.eq(workspace_id))
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
async fn search_asks_a_worker_to_embed_the_query_and_ranks_by_the_result() {
    let (provider, _server, inputs) = keyword_provider(768).await;
    boot_with_query_worker(provider.clone(), |request, ctx| async move {
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Read,
                ..Default::default()
            },
        )
        .await;
        embedded_notes(
            &ctx,
            &setup,
            provider.as_ref(),
            &["alpha plan", "beta plan", "gamma plan"],
        )
        .await;
        let documents = inputs.lock().unwrap().len();
        assert_eq!(documents, 3, "setup embedded three documents");

        let search = |query: &'static str| {
            let request = &request;
            let key = setup.api_key.clone();
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
        let mut queries = inputs.lock().unwrap()[documents..].to_vec();
        queries.sort();
        assert_eq!(
            queries,
            ["alpha", "beta"],
            "each query reaches the HTTP provider once"
        );
        assert_eq!(
            rows_left(&ctx, setup.workspace_id).await,
            0,
            "consumed results are deleted"
        );
    })
    .await;
}

/// With no worker running the search must fail, loudly and boundedly: a lexical fallback would return results the caller did not ask for.
#[tokio::test]
#[serial(process_environment)]
async fn search_without_a_worker_times_out_as_service_unavailable_without_a_fallback() {
    let guard = crate::EnvGuard::capture(&[
        "YORISHIRO_QUERY_EMBEDDING_TIMEOUT_MS",
        "YORISHIRO_QUERY_EMBEDDING_POLL_INTERVAL_MS",
    ]);
    guard.set("YORISHIRO_QUERY_EMBEDDING_TIMEOUT_MS", "300");
    guard.set("YORISHIRO_QUERY_EMBEDDING_POLL_INTERVAL_MS", "50");
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Read,
                ..Default::default()
            },
        )
        .await;
        // An entity whose text matches the query literally, so a lexical fallback would find it.
        let (provider, _server, _) = keyword_provider(768).await;
        embedded_notes(&ctx, &setup, provider.as_ref(), &["alpha plan"]).await;

        let started = std::time::Instant::now();
        let response = request
            .get("/api/search?query_text=alpha")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
        assert_eq!(
            rows_left(&ctx, setup.workspace_id).await,
            0,
            "the expired request is consumed"
        );
    })
    .await;
}

/// A worker that answers after its request expired must not resurrect it or fail.
/// Expire through the production model after the real worker reaches the HTTP provider.
#[tokio::test]
async fn a_result_for_an_expired_request_is_discarded() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::query_embedding_requests::Entity as Requests;
    use yorishiro::services::embedding::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (started, gate) = (entered.clone(), release.clone());
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move || {
            let (started, gate) = (started.clone(), gate.clone());
            async move {
                started.notify_one();
                gate.acquire().await.unwrap().forget();
                let mut vector = vec![0.0_f32; 768];
                vector[0] = 1.0;
                axum::Json(serde_json::json!({"data": [{"index": 0, "embedding": vector}]}))
            }
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: "late-query-test".into(),
        dimensions: 768,
        send_dimensions_param: false,
    });
    let (workspace_tx, workspace_rx) = tokio::sync::oneshot::channel();
    boot_with_query_worker_and_drain(
        Arc::new(provider),
        |request, ctx| async move {
            let setup = TestTenant::create(
                &request,
                &ctx,
                TenantArgs {
                    key_scope: ApiKeyScope::Read,
                    ..Default::default()
                },
            )
            .await;
            let search = async {
                request
                    .get("/api/search?query_text=alpha")
                    .add_header("Authorization", format!("Bearer {}", setup.api_key))
                    .await
            };
            let expire = async {
                tokio::time::timeout(Duration::from_secs(10), entered.notified())
                    .await
                    .expect("the real worker reaches the provider before expiry");
                // Observe the HTTP-created request only to synchronize the expiry fault.
                let pending = Requests::find()
                    .filter(query_embedding_requests::Column::WorkspaceId.eq(setup.workspace_id))
                    .one(&ctx.db)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    Requests::expire(&ctx.db, setup.workspace_id, pending.id)
                        .await
                        .unwrap()
                );
            };
            let (response, ()) = tokio::join!(search, expire);
            assert_eq!(response.status_code(), StatusCode::SERVICE_UNAVAILABLE);
            release.add_permits(1);
            workspace_tx.send(setup.workspace_id).unwrap();
        },
        |ctx| async move {
            // Query deliveries intentionally have no lifecycle; inspect only after Loco drains the worker.
            let workspace_id = workspace_rx.await.unwrap();
            assert_eq!(
                rows_left(&ctx, workspace_id).await,
                0,
                "the late result neither revives nor leaks the request"
            );
        },
    )
    .await;
}

/// A worker that cannot embed records why, and the search reports it as an unavailable service rather than a bad request.
#[tokio::test]
async fn a_worker_failure_reaches_the_caller_as_service_unavailable() {
    let (provider, _server) = unavailable_provider().await;
    boot_with_query_worker(provider, |request, ctx| async move {
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Read,
                ..Default::default()
            },
        )
        .await;
        let response = request
            .get("/api/search?query_text=alpha")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
                .contains("HTTP 503"),
            "the worker's diagnostic is kept: {body}"
        );
        assert!(!body.to_string().contains("private upstream diagnostic"));
        assert_eq!(rows_left(&ctx, setup.workspace_id).await, 0);
    })
    .await;
}

/// A model whose width the workspace's vectors do not have is refused by the worker, against the same registry the writes use.
#[tokio::test]
async fn a_width_the_workspace_does_not_hold_is_refused_by_the_worker() {
    let (provider, _server, _) = keyword_provider(1024).await;
    boot_with_query_worker(provider, |request, ctx| async move {
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Read,
                ..Default::default()
            },
        )
        .await;
        // The workspace already holds 768-wide vectors.
        let (provider, _server, _) = keyword_provider(768).await;
        embedded_notes(&ctx, &setup, provider.as_ref(), &["alpha plan"]).await;

        let response = request
            .get("/api/search?query_text=alpha")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
    })
    .await;
}

/// `YORISHIRO_EMBEDDING_PROVIDER=none` on the worker is an accurate, immediate answer, not a timeout.
#[tokio::test]
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
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Read,
                ..Default::default()
            },
        )
        .await;
        let response = request
            .get("/api/search?query_text=alpha")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
