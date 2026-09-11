use super::boot_request;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::models::{entity_entities, schema_schemas};

use super::fixtures::{self, TenantArgs};

struct Setup {
    tenant_id: Uuid,
    workspace_id: Uuid,
    key: String,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let (tenant_id, workspace_id, _, key) =
        fixtures::create_tenant_workspace_owner(ctx, TenantArgs::default()).await;
    Setup {
        tenant_id,
        workspace_id,
        key,
    }
}

/// The full shape a caller like `ee/`'s fill-proposal confirmation depends on: a job's snapshots restore the entities they cover, and one deleted since the snapshot is counted rather than failing the rest.
#[tokio::test]
#[serial]
async fn undo_restores_snapshotted_entities_and_counts_a_deleted_one() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let definition = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
            }
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

        let survivor = entity_entities::create(
            &ctx.db,
            setup.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "before" }),
            },
            None,
        )
        .await
        .expect("create survivor");
        let doomed = entity_entities::create(
            &ctx.db,
            setup.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "also before" }),
            },
            None,
        )
        .await
        .expect("create doomed");

        let job_id = Uuid::new_v4();
        entity_entities::snapshot(&ctx.db, setup.workspace_id, survivor.id, job_id)
            .await
            .expect("snapshot survivor");
        entity_entities::snapshot(&ctx.db, setup.workspace_id, doomed.id, job_id)
            .await
            .expect("snapshot doomed");

        // Overwrite the survivor, matching what a batch job (fill-defaults, fill-proposal confirmation) does between taking the snapshot and the undo that might follow it.
        entity_entities::update(
            &ctx.db,
            setup.workspace_id,
            survivor.id,
            serde_json::json!({ "title": "overwritten" }),
            None,
        )
        .await
        .expect("overwrite survivor");
        entity_entities::delete(&ctx.db, setup.workspace_id, doomed.id)
            .await
            .expect("delete doomed");

        let response = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(
            response.status_code(),
            200,
            "response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["restored"], 1, "body: {body}");
        assert_eq!(body["missing"], 1, "body: {body}");

        let restored = entity_entities::get(&ctx.db, setup.workspace_id, survivor.id)
            .await
            .expect("read back survivor");
        assert_eq!(restored.data["title"], "before");
        assert_eq!(restored.schema_version, 1);
    })
    .await;
}

/// A job with no snapshots (an unknown or already-undone job id) is refused rather than reporting zero restored, so a caller can tell "nothing to undo" from "already undone".
#[tokio::test]
#[serial]
async fn undo_an_unknown_job_is_refused() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let response = request
            .post(&format!("/api/migration-jobs/{}/undo", Uuid::new_v4()))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(
            response.status_code(),
            404,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}
