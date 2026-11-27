use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

#[tokio::test]
async fn provider_settings_reject_invalid_base_urls_before_saving() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let auth = format!("Bearer {}", tenant.api_key);
        for (path, mut body) in [
            (
                "/api/workspace/embedding-provider",
                serde_json::json!({"model":"test", "api_key":"test-key", "dimensions":32}),
            ),
            (
                "/api/workspace/llm-provider",
                serde_json::json!({"model":"test", "api_key":"test-key"}),
            ),
        ] {
            for base_url in [
                "https://",
                "https://u:p@example.invalid/v1",
                "https://example.invalid/v1?token=x",
            ] {
                body["base_url"] = serde_json::json!(base_url);
                request
                    .put(path)
                    .add_header("Authorization", &auth)
                    .json(&body)
                    .await
                    .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
            }
            let closed = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            body["base_url"] =
                serde_json::json!(format!("http://{}", closed.local_addr().unwrap()));
            drop(closed);
            request
                .put(path)
                .add_header("Authorization", &auth)
                .json(&body)
                .await
                .assert_status(StatusCode::BAD_GATEWAY);
            request
                .get(path)
                .add_header("Authorization", &auth)
                .await
                .assert_status(StatusCode::NOT_FOUND);
        }
    })
    .await;
}
use axum::http::StatusCode;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, SelectExt};
use yorishiro::App;
use yorishiro::models::_entities::queue_job_lifecycles::{Column, Entity};

#[tokio::test]
async fn document_prefix_invalidates_vectors_but_query_instruction_does_not() {
    document_prefix_revision_contract().await;
}

#[tokio::test]
async fn postgres_document_prefix_revision_contract() {
    if !crate::require_postgres_backend() {
        return;
    }
    document_prefix_revision_contract().await;
}

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
async fn document_prefix_revision_contract() {
    use sea_orm::EntityTrait;
    use yorishiro::models::workspace_workspaces;
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let read = || workspace_workspaces::Entity::find_by_id(setup.workspace_id).one(&ctx.db);
        let initial = read().await.unwrap().unwrap();
        let (_, provider, _) = crate::http_stub::keyword_provider(768).await;
        let mut config = serde_json::json!({
            "base_url": provider.url,
            "model": "test-model", "api_key": "test-key", "dimensions": 768,
            "query_instruction": "query: ", "document_prefix": "passage: "
        });
        let put = request
            .put("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&config)
            .await;
        assert_eq!(put.status_code(), StatusCode::NO_CONTENT, "{}", put.text());
        let assigned = read().await.unwrap().unwrap();
        assert!(assigned.embedding_revision > initial.embedding_revision);
        assert!(assigned.embedding_reindexing);

        config["query_instruction"] = serde_json::json!("different query: ");
        let put = request
            .put("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&config)
            .await;
        assert_eq!(put.status_code(), StatusCode::NO_CONTENT, "{}", put.text());
        let query_changed = read().await.unwrap().unwrap();
        assert_eq!(
            query_changed.embedding_revision,
            assigned.embedding_revision
        );
        assert_eq!(
            query_changed.embedding_reindexing,
            assigned.embedding_reindexing
        );

        config["document_prefix"] = serde_json::json!("new passage: ");
        let put = request
            .put("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&config)
            .await;
        assert_eq!(put.status_code(), StatusCode::NO_CONTENT, "{}", put.text());
        let document_changed = read().await.unwrap().unwrap();
        assert!(document_changed.embedding_revision > query_changed.embedding_revision);
        assert!(document_changed.embedding_reindexing);
        let response = request
            .get("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            response.json::<serde_json::Value>()["document_prefix"],
            "new passage: "
        );
        assert!(!response.text().contains("test-key"));
    })
    .await;
}

/// Setting, reading and clearing a workspace's own embedding provider over REST.
/// The key itself never comes back from GET, only what it configured, matching `llm_key_set_get_and_clear_round_trip`.
#[tokio::test]
async fn embedding_key_set_get_and_clear_round_trip() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let (_, provider, _) = crate::http_stub::keyword_provider(1536).await;

        let missing = request
            .get("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            missing.status_code(),
            StatusCode::NOT_FOUND,
            "response: {:?}",
            missing.text()
        );

        let put = request
            .put("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": format!("{}/", provider.url),
                "model": "text-embedding-3-small",
                "api_key": "sk-secret-value",
                "dimensions": 1536
            }))
            .await;
        assert_eq!(
            put.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            put.text()
        );

        let get = request
            .get("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            get.status_code(),
            StatusCode::OK,
            "response: {:?}",
            get.text()
        );
        let body: serde_json::Value = get.json();
        // The trailing slash is trimmed once at write time, matching llm-key.
        assert_eq!(body["base_url"], provider.url);
        assert_eq!(body["model"], "text-embedding-3-small");
        assert_eq!(body["dimensions"], 1536);
        assert_eq!(body["configured"], true);
        let rendered = get.text();
        assert!(
            !rendered.contains("sk-secret-value"),
            "the key must never be returned: {rendered}"
        );

        let delete = request
            .delete("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            delete.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            delete.text()
        );

        let after_delete = request
            .get("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(after_delete.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

/// A scheme that could never be an embeddings endpoint is refused before anything is stored, matching `a_non_http_base_url_is_refused`.
#[tokio::test]
async fn a_non_http_base_url_is_refused() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        for bad_url in [
            "file:///etc/passwd",
            "gopher://example.com",
            "embed.example.com/v1",
        ] {
            let put = request
                .put("/api/workspace/embedding-provider")
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
                .json(&serde_json::json!({
                    "base_url": bad_url,
                    "model": "m",
                    "api_key": "k",
                    "dimensions": 768
                }))
                .await;
            assert_eq!(
                put.status_code(),
                422,
                "{bad_url:?} should have been refused: {:?}",
                put.text()
            );
        }
    })
    .await;
}

/// Assigning a provider whose `dimensions` does not match a workspace's own stamped `embedding_dimensions` is stored, and the key change schedules a reindex of the workspace, rather than being discovered only on the next entity write (`sync_embedding`'s own write-time guard is the backstop behind it).
#[tokio::test]
async fn a_dimension_mismatch_against_the_workspace_stamp_stores_and_triggers_reindex() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let definition = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": { "note": { "fields": {
                "title": { "type": "string", "x-embed": true }
            } } }
        }))
        .expect("parse schema");
        yorishiro::models::schema_schemas::create_schema(
            &ctx.db,
            setup.tenant_id,
            setup.workspace_id,
            definition,
            None,
            None,
        )
        .await
        .expect("create schema");
        let entity = yorishiro::models::entity_entities::create(
            &ctx.db,
            setup.workspace_id,
            yorishiro::models::entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "alpha" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let record =
            yorishiro::models::entity_entities::get(&ctx.db, setup.workspace_id, entity.id)
                .await
                .expect("read entity");
        let (provider, _server, _) = crate::http_stub::keyword_provider(768).await;
        yorishiro::models::entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            setup.workspace_id,
            &record,
            provider.as_ref(),
        )
        .await
        .expect("persist vector and workspace stamp");

        let (_, provider, _) = crate::http_stub::keyword_provider(1024).await;
        let put = request
            .put("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": provider.url,
                "model": "embed-3072",
                "api_key": "sk-secret-value",
                // Deliberately differs from the workspace's stamped width.
                "dimensions": 1024
            }))
            .await;
        assert_eq!(
            put.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            put.text()
        );

        // GET returns the new assignment while durable reindex work is queued.
        assert!(
            Entity::find()
                .filter(Column::WorkspaceId.eq(setup.workspace_id))
                .filter(Column::JobName.eq("reindex"))
                .exists(&ctx.db)
                .await
                .expect("read durable reindex registration")
        );
        let get = request
            .get("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            get.status_code(),
            StatusCode::OK,
            "response: {:?}",
            get.text()
        );
    })
    .await;
}

/// Positive arbitrary widths are accepted without request-time schema changes.
#[tokio::test]
async fn set_accepts_arbitrary_positive_widths_without_creating_tables() {
    use sea_orm::ConnectionTrait;
    use sea_orm::sea_query::{Alias, Asterisk, Expr, ExprTrait, Func, Query};

    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let (_, provider, _) = crate::http_stub::keyword_provider(3072).await;

        let refused = request
            .put("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": provider.url,
                "model": "text-embedding-3-large",
                "api_key": "sk-secret-value",
                "dimensions": 0
            }))
            .await;
        assert_eq!(
            refused.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            refused.text()
        );
        assert!(
            request
                .get("/api/workspace/embedding-provider")
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
                .await
                .status_code()
                == StatusCode::NOT_FOUND,
            "a refused assignment must store nothing"
        );

        let stored = request
            .put("/api/workspace/embedding-provider")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": provider.url,
                "model": "embed-3072",
                "api_key": "sk-secret-value",
                "dimensions": 3072
            }))
            .await;
        assert_eq!(
            stored.status_code(),
            StatusCode::NO_CONTENT,
            "{}",
            stored.text()
        );

        // The catalogue that lists tables differs per backend.
        let (catalogue, name_column) =
            if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                (Alias::new("sqlite_master"), "name")
            } else {
                (Alias::new("tables"), "table_name")
            };
        let mut stray_tables = Query::select();
        stray_tables
            .expr_as(Func::count(Expr::col(Asterisk)), Alias::new("n"))
            .and_where(Expr::col(Alias::new(name_column)).like("content_entity_embeddings%"));
        if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            stray_tables
                .from(catalogue)
                .and_where(Expr::col(Alias::new("type")).eq("table"));
        } else {
            stray_tables.from((Alias::new("information_schema"), catalogue));
        }
        let stray: i64 = ctx
            .db
            .query_one(&stray_tables)
            .await
            .expect("query tables")
            .expect("count row")
            .try_get("", "n")
            .expect("count value");
        assert_eq!(stray, 0, "no table outside the community naming may exist");
    })
    .await;
}
