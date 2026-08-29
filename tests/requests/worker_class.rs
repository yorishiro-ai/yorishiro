use loco_rs::testing::prelude::*;
use sea_orm::EntityTrait;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::ee::services::worker_class_resolver::WorkerClassAssignmentResolver;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;
use yorishiro::workers::embedding_sync::WorkerClassResolver;

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

/// Setting, reading and clearing a workspace's own worker-class assignment over REST, matching `embedding_key_set_get_and_clear_round_trip`.
#[tokio::test]
#[serial]
async fn worker_class_set_get_and_clear_round_trip() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let missing = request
            .get("/hosted/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(missing.status_code(), 404, "response: {:?}", missing.text());

        let put = request
            .put("/hosted/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({ "worker_class": "tenant_private" }))
            .await;
        assert_eq!(put.status_code(), 204, "response: {:?}", put.text());

        let get = request
            .get("/hosted/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(get.status_code(), 200, "response: {:?}", get.text());
        let body: serde_json::Value = get.json();
        assert_eq!(body["worker_class"], "tenant_private");

        let delete = request
            .delete("/hosted/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(delete.status_code(), 204, "response: {:?}", delete.text());

        let after_delete = request
            .get("/hosted/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(after_delete.status_code(), 404);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// Re-`PUT`ting a different class replaces the assignment rather than erroring or adding a second row, matching the `ON CONFLICT` upsert `embedding_keys::set`/`llm_keys::set` both use.
#[tokio::test]
#[serial]
async fn setting_a_new_class_replaces_the_old_one() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let first = request
            .put("/hosted/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({ "worker_class": "tenant_private" }))
            .await;
        assert_eq!(first.status_code(), 204, "response: {:?}", first.text());

        let second = request
            .put("/hosted/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({ "worker_class": "official" }))
            .await;
        assert_eq!(second.status_code(), 204, "response: {:?}", second.text());

        let get = request
            .get("/hosted/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let body: serde_json::Value = get.json();
        assert_eq!(
            body["worker_class"], "official",
            "the second PUT must replace the first, not add a second row"
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// The `WorkerClassResolver` seam returns the workspace's own assignment when one exists, and `None` (falling back to `WorkerClass::Shared`) when it does not, matching `resolver_returns_the_workspace_assignment_when_set_and_none_otherwise`.
#[tokio::test]
#[serial]
async fn resolver_returns_the_workspace_assignment_when_set_and_none_otherwise() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
        let setup = setup(&ctx).await;
        let resolver = WorkerClassAssignmentResolver;

        let before = resolver
            .resolve(&ctx.db, setup.workspace_id)
            .await
            .expect("resolve before assignment");
        assert!(
            before.is_none(),
            "an unassigned workspace must resolve to None so the caller falls back to WorkerClass::Shared"
        );

        yorishiro::ee::models::worker_classes::set(
            &ctx.db,
            setup.workspace_id,
            yorishiro::workers::embedding_sync::WorkerClass::Official,
        )
        .await
        .expect("assign workspace worker class");

        let after = resolver
            .resolve(&ctx.db, setup.workspace_id)
            .await
            .expect("resolve after assignment");
        assert_eq!(
            after,
            Some(yorishiro::workers::embedding_sync::WorkerClass::Official),
            "the resolved class must be the one just assigned"
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

        super::close_app_pools(&ctx).await;
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
