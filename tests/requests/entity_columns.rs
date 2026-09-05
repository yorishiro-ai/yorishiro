use super::boot_request;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

struct Setup {
    key: String,
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
        ApiKeyScope::Write,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue key")
    .plaintext;
    Setup { key }
}

/// A workspace that never chose reads as absent (an empty list, not a 404, since `list` is "every stored preference" and there being none is not an error); setting, then resetting, round-trips through the exact stored order and back to absence.
#[tokio::test]
#[serial]
async fn set_get_and_reset_round_trip_in_display_order() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let before = request
            .get("/api/workspace/entity-columns")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(before.status_code(), 200, "response: {:?}", before.text());
        assert!(before.json::<Vec<serde_json::Value>>().is_empty());

        let put = request
            .put("/api/workspace/entity-columns/task")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({ "columns": ["priority", "title", "done"] }))
            .await;
        assert_eq!(put.status_code(), 200, "response: {:?}", put.text());
        let body: serde_json::Value = put.json();
        assert_eq!(
            body["columns"],
            serde_json::json!(["priority", "title", "done"])
        );

        let after = request
            .get("/api/workspace/entity-columns")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let stored: Vec<serde_json::Value> = after.json();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0]["entity_type"], "task");
        assert_eq!(
            stored[0]["columns"],
            serde_json::json!(["priority", "title", "done"])
        );

        // Saving again replaces rather than appending: one row, the new order.
        let put_again = request
            .put("/api/workspace/entity-columns/task")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({ "columns": ["done", "title"] }))
            .await;
        assert_eq!(put_again.status_code(), 200);
        let after_again = request
            .get("/api/workspace/entity-columns")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let stored_again: Vec<serde_json::Value> = after_again.json();
        assert_eq!(
            stored_again.len(),
            1,
            "must replace, not append: {stored_again:?}"
        );
        assert_eq!(
            stored_again[0]["columns"],
            serde_json::json!(["done", "title"])
        );

        let reset = request
            .delete("/api/workspace/entity-columns/task")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(reset.status_code(), 204, "response: {:?}", reset.text());

        let after_reset = request
            .get("/api/workspace/entity-columns")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert!(after_reset.json::<Vec<serde_json::Value>>().is_empty());
    })
    .await;
}

/// A duplicate field would render twice and make reordering ambiguous, so it is refused rather than silently deduplicated; more than the maximum is refused for the same "the table stays a table" reason.
/// Neither refusal leaves a row behind.
#[tokio::test]
#[serial]
async fn a_duplicate_or_over_limit_selection_is_refused_and_leaves_no_row() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let duplicate = request
            .put("/api/workspace/entity-columns/task")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({ "columns": ["title", "title"] }))
            .await;
        assert_eq!(
            duplicate.status_code(),
            422,
            "response: {:?}",
            duplicate.text()
        );

        let over_limit: Vec<String> = (0
            ..yorishiro::ee::models::entity_columns::MAX_VISIBLE_COLUMNS + 1)
            .map(|i| format!("f{i}"))
            .collect();
        let too_many = request
            .put("/api/workspace/entity-columns/task")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({ "columns": over_limit }))
            .await;
        assert_eq!(
            too_many.status_code(),
            422,
            "response: {:?}",
            too_many.text()
        );

        let after = request
            .get("/api/workspace/entity-columns")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert!(
            after.json::<Vec<serde_json::Value>>().is_empty(),
            "neither refused save should have left a row"
        );
    })
    .await;
}

/// An explicit empty selection is a choice ("show nothing") and is stored as a row, distinct from having never chosen at all.
#[tokio::test]
#[serial]
async fn an_empty_selection_is_stored_as_a_choice() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let put = request
            .put("/api/workspace/entity-columns/task")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({ "columns": [] }))
            .await;
        assert_eq!(put.status_code(), 200, "response: {:?}", put.text());

        let after = request
            .get("/api/workspace/entity-columns")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let stored: Vec<serde_json::Value> = after.json();
        assert_eq!(
            stored.len(),
            1,
            "an empty choice must still be a row: {stored:?}"
        );
        assert!(stored[0]["columns"].as_array().unwrap().is_empty());
    })
    .await;
}
