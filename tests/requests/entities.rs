use super::boot_request;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::{entity_entities, schema_schemas};

use super::fixtures::{TenantArgs, TestTenant};

#[cfg(feature = "enterprise")]
#[tokio::test]
#[serial_test::serial(process_environment)]
async fn entity_plan_quota_expires_but_manual_limit_remains() {
    use axum::http::StatusCode;
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};
    use std::sync::Arc;
    use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
    let variable = crate::env::TestEnv::PlanPolicyEnabled.name();
    let guard = crate::EnvGuard::capture(&[variable]);
    guard.set(variable, "true");
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let definition = serde_json::from_value(serde_json::json!({
            "name":"quota-note", "entity_types":{"note":{"fields":{"title":{"type":"string"}}}}
        })).unwrap();
        let (schema, _) = schema_schemas::create_schema(&ctx.db, tenant.tenant_id, tenant.workspace_id, definition, None, None).await.unwrap();
        // Seed exactly at Free's boundary without hundreds of embedding jobs.
        entity_entities::Entity::insert_many((0..500).map(|index| entity_entities::ActiveModel {
            id: Set(Uuid::now_v7()), workspace_id: Set(tenant.workspace_id), schema_id: Set(schema.id),
            schema_version: Set(schema.version), entity_type: Set("note".into()),
            data: Set(serde_json::json!({"title":format!("seed-{index}")})), ..Default::default()
        })).exec(&ctx.db).await.unwrap();
        let licence = |exp| ctx.shared_store.insert(Arc::new(LicenceState::licensed(LicenceClaims {
            sub:"entity-quota".into(), plan:"free".into(), exp,
        })));
        let body = serde_json::json!({"schema_name":"quota-note","entity_type":"note","data":{"title":"new"}});
        licence(chrono::Utc::now().timestamp() + 3600);
        request.post("/api/entities").add_header("Authorization", format!("Bearer {}",tenant.api_key))
            .json(&body).await.assert_status(StatusCode::CONFLICT);
        use sea_orm::{ColumnTrait, QueryFilter};
        let seed = entity_entities::Entity::find()
            .filter(yorishiro::models::_entities::entity_entities::Column::WorkspaceId.eq(tenant.workspace_id))
            .one(&ctx.db).await.unwrap().unwrap();
        let jsonl = serde_json::to_string(&serde_json::json!({"kind":"entity", "record":entity_entities::EntityRecord::from(seed)})).unwrap();
        let import = request.post("/api/import.jsonl").add_header("Authorization", format!("Bearer {}",tenant.api_key))
            .text(jsonl).await;
        import.assert_status(StatusCode::ACCEPTED);
        let id: Uuid = import.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        super::fill_defaults::fail_via_api(&request, &ctx, id, &tenant.api_key, &format!("/api/import-jobs/{id}")).await;
        assert_eq!(entity_entities::count(&ctx.db, tenant.workspace_id).await.unwrap(), 500);
        licence(chrono::Utc::now().timestamp() - 1);
        request.post("/api/entities").add_header("Authorization", format!("Bearer {}",tenant.api_key))
            .json(&body).await.assert_status(StatusCode::CREATED);
        let _ = ctx.shared_store.remove::<Arc<LicenceState>>();
        request.post("/api/entities").add_header("Authorization", format!("Bearer {}",tenant.api_key))
            .json(&body).await.assert_status(StatusCode::CREATED);
        yorishiro::models::workspace_workspaces::ActiveModel {
            id:Set(tenant.workspace_id), max_entities:Set(Some(0)), ..Default::default()
        }.update(&ctx.db).await.unwrap();
        request.post("/api/entities").add_header("Authorization", format!("Bearer {}",tenant.api_key))
            .json(&body).await.assert_status(StatusCode::CONFLICT);
    }).await;
}

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
