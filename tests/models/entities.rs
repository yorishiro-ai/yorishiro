use crate::requests::boot_request;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::{content_entities, content_schemas};

/// `migration_dry_run` uses `select_only().group_by(...).into_model::<GroupedCount>()`.
/// A single-version workspace can't tell whether that mapping is correct: every entity lands in the `current` bucket and the per-old-version loop body never runs.
/// This test forces a second version so the loop actually executes.
#[tokio::test]
#[serial]
async fn drift_and_migration_dry_run_see_a_second_version() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("drift-test".into()),
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

        // v1: one required field.
        let v1: serde_json::Value = serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": {
                    "fields": {
                        "title": { "type": "string", "required": true }
                    }
                }
            }
        });
        let v1_def = serde_json::from_value(v1).expect("parse v1");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, v1_def, None, None)
            .await
            .expect("create v1");

        let entity = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "written against v1" }),
            },
            None,
        )
        .await
        .expect("create entity against v1");

        // v2: adds a new required field. The v1 entity above now predates it.
        let v2: serde_json::Value = serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": {
                    "fields": {
                        "title": { "type": "string", "required": true },
                        "body": { "type": "string", "required": true }
                    }
                }
            }
        });
        let v2_def = serde_json::from_value(v2).expect("parse v2");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, v2_def, None, None)
            .await
            .expect("create v2");

        let drift = content_entities::drift(&ctx.db, workspace.id, entity.id)
            .await
            .expect("drift");
        assert_eq!(drift.schema_version, 1);
        assert_eq!(drift.active_schema_version, 2);
        assert_eq!(drift.missing_fields.len(), 1, "drift: {drift:?}");
        assert_eq!(drift.missing_fields[0].name, "body");
        assert!(drift.missing_fields[0].required);

        let report = content_entities::migration_dry_run(&ctx.db, workspace.id, "note")
            .await
            .expect("migration_dry_run");
        assert_eq!(report.active_version, 2);
        assert_eq!(report.total_entities, 1);
        assert_eq!(report.current, 0, "report: {report:?}");
        assert_eq!(report.behind_but_valid, 0);
        assert_eq!(report.needs_values, 1);
        assert_eq!(report.by_entity_type.len(), 1);
        let by_type = &report.by_entity_type[0];
        assert_eq!(by_type.entity_type, "note");
        assert_eq!(by_type.behind, 1);
        assert_eq!(by_type.needs_values, 1);
        assert_eq!(by_type.missing_required, vec!["body".to_string()]);
    })
    .await;
}
