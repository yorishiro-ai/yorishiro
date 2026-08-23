use loco_rs::testing::prelude::*;
use serial_test::serial;
use uuid::Uuid;
use yorishiro_core::app::App;
use yorishiro_core::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::models::{content_entities, content_schemas};
use yorishiro_core::services::auth::ApiKeyScope;

struct Setup {
    tenant_id: Uuid,
    workspace_id: Uuid,
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
    // Migration scope: undoing a batch is a migration operation, above schema in the ladder.
    let key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Migration,
        Some(owner.id),
    )
    .await
    .expect("issue key")
    .plaintext;
    Setup {
        tenant_id: tenant.id,
        workspace_id: workspace.id,
        key,
    }
}

/// The full shape a caller like `ee/`'s fill-proposal confirmation depends on: a job's snapshots
/// restore the entities they cover, and one deleted since the snapshot is counted rather than
/// failing the rest.
#[tokio::test]
#[serial]
async fn undo_restores_snapshotted_entities_and_counts_a_deleted_one() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let definition = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
            }
        }))
        .expect("parse definition");
        content_schemas::create_schema(
            &ctx.db,
            setup.tenant_id,
            setup.workspace_id,
            definition,
            None,
            None,
        )
        .await
        .expect("create schema");

        let survivor = content_entities::create(
            &ctx.db,
            setup.workspace_id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "before" }),
            },
            None,
        )
        .await
        .expect("create survivor");
        let doomed = content_entities::create(
            &ctx.db,
            setup.workspace_id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "also before" }),
            },
            None,
        )
        .await
        .expect("create doomed");

        let job_id = Uuid::new_v4();
        content_entities::snapshot(&ctx.db, setup.workspace_id, survivor.id, job_id)
            .await
            .expect("snapshot survivor");
        content_entities::snapshot(&ctx.db, setup.workspace_id, doomed.id, job_id)
            .await
            .expect("snapshot doomed");

        // Overwrite the survivor, matching what a batch job (fill-defaults, fill-proposal
        // confirmation) does between taking the snapshot and the undo that might follow it.
        content_entities::update(
            &ctx.db,
            setup.workspace_id,
            survivor.id,
            serde_json::json!({ "title": "overwritten" }),
            None,
        )
        .await
        .expect("overwrite survivor");
        content_entities::delete(&ctx.db, setup.workspace_id, doomed.id)
            .await
            .expect("delete doomed");

        let response = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(response.status_code(), 200, "response: {:?}", response.text());
        let body: serde_json::Value = response.json();
        assert_eq!(body["restored"], 1, "body: {body}");
        assert_eq!(body["missing"], 1, "body: {body}");

        let restored = content_entities::get(&ctx.db, setup.workspace_id, survivor.id)
            .await
            .expect("read back survivor");
        assert_eq!(restored.data["title"], "before");
        assert_eq!(restored.schema_version, 1);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A job with no snapshots (an unknown or already-undone job id) is refused rather than reporting
/// zero restored, so a caller can tell "nothing to undo" from "already undone".
#[tokio::test]
#[serial]
async fn undo_an_unknown_job_is_refused() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let response = request
            .post(&format!("/api/migration-jobs/{}/undo", Uuid::new_v4()))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(response.status_code(), 404, "response: {:?}", response.text());

        super::close_app_pools(&ctx).await;
    })
    .await;
}
