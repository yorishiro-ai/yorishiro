use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use yorishiro::App;
use yorishiro::models::{entity_entities, schema_schemas};

/// `migration_dry_run` uses `select_only().group_by(...).into_model::<GroupedCount>()`.
/// A single-version workspace can't tell whether that mapping is correct: every entity lands in the `current` bucket and the per-old-version loop body never runs.
/// This test forces a second version so the loop actually executes.
#[tokio::test]
async fn drift_and_migration_dry_run_see_a_second_version() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

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
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            v1_def,
            None,
            None,
        )
        .await
        .expect("create v1");

        let entity = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
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
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            v2_def,
            None,
            None,
        )
        .await
        .expect("create v2");

        let drift = entity_entities::drift(&ctx.db, fixture.workspace_id, entity.id)
            .await
            .expect("drift");
        assert_eq!(drift.schema_version, 1);
        assert_eq!(drift.active_schema_version, 2);
        assert_eq!(drift.missing_fields.len(), 1, "drift: {drift:?}");
        assert_eq!(drift.missing_fields[0].name, "body");
        assert!(drift.missing_fields[0].required);

        let report = entity_entities::migration_dry_run(&ctx.db, fixture.workspace_id, "note")
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
