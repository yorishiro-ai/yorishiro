use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant};
use axum::http::StatusCode;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

/// Setting, reading and clearing a workspace's own embedding provider over REST.
/// The key itself never comes back from GET, only what it configured, matching `llm_key_set_get_and_clear_round_trip`.
#[tokio::test]
async fn embedding_key_set_get_and_clear_round_trip() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

        let missing = request
            .get("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            missing.status_code(),
            StatusCode::NOT_FOUND,
            "response: {:?}",
            missing.text()
        );

        let put = request
            .put("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": "https://embed.example.com/v1/",
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
            .get("/api/workspace/embedding-key")
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
        assert_eq!(body["base_url"], "https://embed.example.com/v1");
        assert_eq!(body["model"], "text-embedding-3-small");
        assert_eq!(body["dimensions"], 1536);
        assert_eq!(body["configured"], true);
        let rendered = get.text();
        assert!(
            !rendered.contains("sk-secret-value"),
            "the key must never be returned: {rendered}"
        );

        let delete = request
            .delete("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            delete.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            delete.text()
        );

        let after_delete = request
            .get("/api/workspace/embedding-key")
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
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

        for bad_url in [
            "file:///etc/passwd",
            "gopher://example.com",
            "embed.example.com/v1",
        ] {
            let put = request
                .put("/api/workspace/embedding-key")
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
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

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

        let put = request
            .put("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": "https://embed.example.com/v1",
                "model": "embed-1024",
                "api_key": "sk-secret-value",
                // Deliberately not 768, the workspace's own stamp, but a width with an embedding table.
                "dimensions": 1024
            }))
            .await;
        assert_eq!(
            put.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            put.text()
        );

        // Stored with WidthChanged: GET now returns the new assignment.
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, SelectExt};
        use yorishiro::models::_entities::queue_job_lifecycles::{Column, Entity};
        assert!(
            Entity::find()
                .filter(Column::WorkspaceId.eq(setup.workspace_id))
                .filter(Column::JobName.eq("reindex"))
                .exists(&ctx.db)
                .await
                .expect("read durable reindex registration")
        );
        let get = request
            .get("/api/workspace/embedding-key")
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

/// `set` takes the width list from the community edition's embedding tables instead of keeping its own.
/// A width with no table is refused up front and stores nothing, and a supported width stores the key without the old request-path DDL creating a stray `content_entity_embeddings_*` table.
#[tokio::test]
async fn set_checks_the_width_against_the_community_embedding_tables() {
    use sea_orm::{ConnectionTrait, Statement};

    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs { key_scope: ApiKeyScope::Schema, ..Default::default() }).await;

        let refused = request
            .put("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": "https://embed.example.com/v1",
                "model": "text-embedding-3-large",
                "api_key": "sk-secret-value",
                "dimensions": 3072
            }))
            .await;
        assert_eq!(refused.status_code(), StatusCode::UNPROCESSABLE_ENTITY, "{}", refused.text());
        assert!(
            request
                .get("/api/workspace/embedding-key")
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
                .await
                .status_code() == StatusCode::NOT_FOUND,
            "a refused assignment must store nothing"
        );

        let stored = request
            .put("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&serde_json::json!({
                "base_url": "https://embed.example.com/v1",
                "model": "embed-1024",
                "api_key": "sk-secret-value",
                "dimensions": 1024
            }))
            .await;
        assert_eq!(stored.status_code(), StatusCode::NO_CONTENT, "{}", stored.text());

        let backend = ctx.db.get_database_backend();
        let stray_tables = if backend == sea_orm::DatabaseBackend::Sqlite {
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'content_entity_embeddings%'"
        } else {
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_name LIKE 'content_entity_embeddings%'"
        };
        let stray: i64 = ctx
            .db
            .query_one_raw(Statement::from_string(backend, stray_tables))
            .await
            .expect("query tables")
            .expect("count row")
            .try_get_by_index(0)
            .expect("count value");
        assert_eq!(stray, 0, "no table outside the community naming may exist");
    })
    .await;
}
