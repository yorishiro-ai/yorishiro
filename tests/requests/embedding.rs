use loco_rs::testing::prelude::*;
use sea_orm::EntityTrait;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::ee::services::embedding_resolver::EmbeddingKeyResolver;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;
use yorishiro::services::embedding::WorkspaceEmbeddingResolver;

struct Setup {
    key: String,
    workspace_id: Uuid,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("acme".into()),
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
    let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
        .await
        .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");
    let key = identity_api_keys::Entity::create_api_key(
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
#[serial]
async fn embedding_key_set_get_and_clear_round_trip() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let missing = request
            .get("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(missing.status_code(), 404, "response: {:?}", missing.text());

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
        assert_eq!(put.status_code(), 204, "response: {:?}", put.text());

        let get = request
            .get("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(get.status_code(), 200, "response: {:?}", get.text());
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
        assert_eq!(delete.status_code(), 204, "response: {:?}", delete.text());

        let after_delete = request
            .get("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(after_delete.status_code(), 404);

    })
    .await;
}

/// A scheme that could never be an embeddings endpoint is refused before anything is stored, matching `a_non_http_base_url_is_refused`.
#[tokio::test]
#[serial]
async fn a_non_http_base_url_is_refused() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
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
#[serial]
async fn a_dimension_mismatch_against_the_workspace_stamp_is_refused_at_config_time() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        // The workspace was created via ActiveModel::insert directly (not POST /setup), so it
        // carries no embedding_dimensions stamp yet; stamp it explicitly to exercise the check.
        let mut active: identity_workspaces::ActiveModel =
            identity_workspaces::Entity::find_by_id(setup.workspace_id)
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
                "model": "text-embedding-3-large",
                "api_key": "sk-secret-value",
                // Deliberately not 768, the workspace's own stamp.
                "dimensions": 3072
            }))
            .await;
        assert_eq!(put.status_code(), 422, "response: {:?}", put.text());

        // Refused, so nothing was stored: GET still reports unconfigured.
        let get = request
            .get("/api/workspace/embedding-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(get.status_code(), 404, "response: {:?}", get.text());

    })
    .await;
}

/// The `WorkspaceEmbeddingResolver` seam returns the workspace's own assignment when one exists, and `None` (falling back to the deployment default) when it does not: the two outcomes every caller of `resolve_embedding_provider` branches on.
#[tokio::test]
#[serial]
async fn resolver_returns_the_workspace_assignment_when_set_and_none_otherwise() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
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

        yorishiro::ee::models::embedding_keys::set(
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
    let first_workspace = identity_workspaces::Entity::find_by_id(first.workspace_id)
        .one(&ctx.db)
        .await
        .expect("find first workspace")
        .expect("first workspace exists");
    let workspace = identity_workspaces::ActiveModel {
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
