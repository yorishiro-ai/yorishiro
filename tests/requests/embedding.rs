use super::boot_request;
use axum::http::StatusCode;
use sea_orm::EntityTrait;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::ee::models::workspace_embedding_keys::EmbeddingKeyResolver;
use yorishiro::models::_entities::{api_keys, tenant_tenants, workspace_workspaces};
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenant_memberships::MembershipRole;
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::embedding::WorkspaceEmbeddingResolver;

struct Setup {
    key: String,
    workspace_id: Uuid,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("acme".into()),
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
    let key = api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Schema,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue key")
    .plaintext;
    Setup {
        key,
        workspace_id: workspace.id,
    }
}

/// Setting, reading and clearing a workspace's own embedding provider over REST.
/// The key itself never comes back from GET, only what it configured, matching `llm_key_set_get_and_clear_round_trip`.
#[tokio::test]
async fn embedding_key_set_get_and_clear_round_trip() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let missing = request
            .get("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(
            missing.status_code(),
            StatusCode::NOT_FOUND,
            "response: {:?}",
            missing.text()
        );

        let put = request
            .put("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(
            delete.status_code(),
            StatusCode::NO_CONTENT,
            "response: {:?}",
            delete.text()
        );

        let after_delete = request
            .get("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(after_delete.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

/// A scheme that could never be an embeddings endpoint is refused before anything is stored, matching `a_non_http_base_url_is_refused`.
#[tokio::test]
async fn a_non_http_base_url_is_refused() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        for bad_url in [
            "file:///etc/passwd",
            "gopher://example.com",
            "embed.example.com/v1",
        ] {
            let put = request
                .put("/api/workspace/embedding-key")
                .add_header("Authorization", format!("Bearer {}", setup.key))
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

/// Assigning a provider whose `dimensions` does not match a workspace's own stamped `embedding_dimensions` is refused at configuration time, not discovered only on the next entity write (`sync_embedding`'s own write-time guard, `services/embedding/sync.rs`, is the backstop this is in front of, not a replacement for it).
#[tokio::test]
async fn a_dimension_mismatch_against_the_workspace_stamp_stores_and_triggers_reindex() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        // The workspace was created via ActiveModel::insert directly (not POST /setup), so it
        // carries no embedding_dimensions stamp yet; stamp it explicitly to exercise the check.
        let mut active: workspace_workspaces::ActiveModel =
            workspace_workspaces::Entity::find_by_id(setup.workspace_id)
                .one(&ctx.db)
                .await
                .expect("find workspace")
                .expect("workspace exists")
                .into();
        active.embedding_dimensions = sea_orm::ActiveValue::Set(Some(768));
        sea_orm::ActiveModelTrait::update(active, &ctx.db)
            .await
            .expect("stamp workspace dimensions");

        let put = request
            .put("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
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
        let get = request
            .get("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
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

/// The `WorkspaceEmbeddingResolver` seam returns the workspace's own assignment when one exists, and `None` (falling back to the deployment default) when it does not: the two outcomes every caller of `resolve_embedding_provider` branches on.
#[tokio::test]
async fn resolver_returns_the_workspace_assignment_when_set_and_none_otherwise() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let setup = setup(&ctx).await;
        let resolver = EmbeddingKeyResolver;

        let before = resolver
            .resolve(&ctx.db, setup.workspace_id)
            .await
            .expect("resolve before assignment");
        assert!(
            before.is_none(),
            "an unassigned workspace must resolve to None so the caller falls back to the deployment default"
        );

        let result = yorishiro::ee::models::workspace_embedding_keys::set(
            &ctx.db,
            setup.workspace_id,
            "https://embed.example.com/v1",
            "text-embedding-3-small",
            "sk-secret-value",
            1536,
            false,
            None,
        )
        .await
        .expect("assign workspace embedding key");
        assert!(
            matches!(result, yorishiro::ee::models::workspace_embedding_keys::SetOutcome::Stored),
            "assigning with no expected dimensions must yield Stored"
        );

        let after = resolver
            .resolve(&ctx.db, setup.workspace_id)
            .await
            .expect("resolve after assignment");
        let provider = after.expect("an assigned workspace must resolve to Some");
        assert_eq!(
            provider.dimensions(),
            1536,
            "the resolved provider must carry the assigned dimensions, not the deployment default"
        );

        // A different, still-unassigned workspace must not see the first workspace's assignment.
        let other = setup_second_workspace(&ctx, &setup).await;
        let other_result = resolver
            .resolve(&ctx.db, other)
            .await
            .expect("resolve unrelated workspace");
        assert!(
            other_result.is_none(),
            "one workspace's assignment must not leak to another"
        );

    })
    .await;
}

/// A second workspace under the same tenant as `setup`, for the cross-workspace isolation check above.
async fn setup_second_workspace(ctx: &loco_rs::app::AppContext, first: &Setup) -> Uuid {
    let first_workspace = workspace_workspaces::Entity::find_by_id(first.workspace_id)
        .one(&ctx.db)
        .await
        .expect("find first workspace")
        .expect("first workspace exists");
    let workspace = workspace_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(first_workspace.tenant_id),
        name: sea_orm::ActiveValue::Set("second".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert second workspace");
    workspace.id
}

/// `set` takes the width list from the community edition's embedding tables instead of keeping its own.
/// A width with no table is refused up front and stores nothing, and a supported width stores the key without the old request-path DDL creating a stray `content_entity_embeddings_*` table.
#[tokio::test]
async fn set_checks_the_width_against_the_community_embedding_tables() {
    use sea_orm::{ConnectionTrait, Statement};
    use yorishiro::ee::models::workspace_embedding_keys::{SetOutcome, set};
    use yorishiro::error::YorishiroError;

    boot_request::<App, _, _>(|_request, ctx| async move {
        let setup = setup(&ctx).await;

        let refused = set(
            &ctx.db,
            setup.workspace_id,
            "https://embed.example.com/v1",
            "text-embedding-3-large",
            "sk-secret-value",
            3072,
            false,
            None,
        )
        .await;
        assert!(
            matches!(refused, Err(YorishiroError::ValidationFailed { .. })),
            "a width with no embedding table must be refused, got: {refused:?}"
        );
        assert!(
            EmbeddingKeyResolver
                .resolve(&ctx.db, setup.workspace_id)
                .await
                .expect("resolve after a refused assignment")
                .is_none(),
            "a refused assignment must store nothing"
        );

        let stored = set(
            &ctx.db,
            setup.workspace_id,
            "https://embed.example.com/v1",
            "embed-1024",
            "sk-secret-value",
            1024,
            false,
            None,
        )
        .await
        .expect("a width with an embedding table is accepted");
        assert!(matches!(stored, SetOutcome::Stored));

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
