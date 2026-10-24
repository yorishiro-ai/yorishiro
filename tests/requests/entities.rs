use super::boot_request;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::{entity_entities, schema_schemas};

use super::fixtures::{TenantArgs, TestTenant};

/// The full shape a caller like `ee/`'s fill-proposal confirmation depends on: a job's snapshots restore the entities they cover, and one deleted since the snapshot is counted rather than failing the rest.
#[tokio::test]
async fn undo_restores_snapshotted_entities_and_counts_a_deleted_one() {
    super::fill_defaults::boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

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
            entity_entities::UpdateEntityInput {
                id: survivor.id,
                data: serde_json::json!({ "title": "overwritten" }),
                updated_by: None,
            },
        )
        .await
        .expect("overwrite survivor");
        entity_entities::delete(&ctx.db, setup.workspace_id, doomed.id)
            .await
            .expect("delete doomed");

        let response = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            response.status_code(),
            202,
            "response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["snapshot_job_id"], job_id.to_string());
        let status_path = format!("/api/migration-jobs/{}", body["job_id"].as_str().unwrap());
        let pending = request
            .get(&status_path)
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(pending.status_code(), 200);
        assert_eq!(pending.json::<serde_json::Value>()["status"], "queued");
        assert!(pending.json::<serde_json::Value>()["result"].is_null());
        let before = entity_entities::get(&ctx.db, setup.workspace_id, survivor.id)
            .await
            .unwrap();
        assert_eq!(before.data["title"], "overwritten");
        super::fill_defaults::finish_via_api(
            &request,
            &ctx,
            body["job_id"].as_str().unwrap().parse().unwrap(),
            &setup.api_key,
            &status_path,
        )
        .await;
        let completed = request
            .get(&status_path)
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(completed.status_code(), 200);
        let completed = completed.json::<serde_json::Value>();
        assert_eq!(completed["status"], "completed");
        assert_eq!(completed["result"]["restored"], 1);
        assert_eq!(completed["result"]["missing"], 1);

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
async fn undo_an_unknown_job_is_refused() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .post(&format!("/api/migration-jobs/{}/undo", Uuid::new_v4()))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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

/// JSONB containment filter on `GET /api/entities?filter=...` must return HTTP 501
/// (`BackendUnsupported`) on SQLite, because the column is `TEXT` there and the
/// JSONB `@>` operator is PostgreSQL-only.
///
/// The model layer returns `YorishiroError::BackendUnsupported`, which the controller
/// renders as HTTP 501 with code `backend_unsupported`.
#[tokio::test]
async fn list_entities_with_filter_returns_501_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

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

        entity_entities::create(
            &ctx.db,
            setup.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "test", "status": "active" }),
            },
            None,
        )
        .await
        .expect("create entity");

        // Query with a filter: should return 501 on SQLite.
        // The filter parameter is raw JSON; axum deserializes the raw query value
        // directly, so we pass it URL-encoded to avoid query-string parsing issues.
        let response = request
            .get("/api/entities?filter=%7B%22status%22:%22active%22%7D")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            response.status_code(),
            501,
            "filter query on SQLite must return 501, response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(
            body["error"]["code"], "backend_unsupported",
            "response must have backend_unsupported code, body: {body}"
        );
    })
    .await;
}
