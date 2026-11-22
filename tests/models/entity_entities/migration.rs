use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use yorishiro::App;
use yorishiro::models::{entity_entities, schema_schemas};

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one schema history compares missing fields, legacy nulls, escaped nested paths, and aggregate migration diagnostics"
)]
async fn migration_diagnostics_inspect_each_stored_body_and_nested_required_fields() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let fields = serde_json::json!({
            "title": {"type":"string", "required":true},
            "category": {"type":"string"},
            "meta": {"type":"object", "properties": {"a/b~c": {"type":"string"}}}
        });
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            serde_json::from_value(
                serde_json::json!({"name":"note", "entity_types":{"note":{"fields":fields}}}),
            )
            .unwrap(),
            None,
            None,
        )
        .await
        .unwrap();
        let mut entities = Vec::new();
        for data in [
            serde_json::json!({"title":"missing", "meta":{}}),
            serde_json::json!({"title":"filled", "category":"general", "meta":{"a/b~c":"value"}}),
            serde_json::json!({"title":"present null", "category":null, "meta":null}),
        ] {
            // Nulls are deliberately injected as legacy data to verify presence, not type validation.
            let mut valid = data.clone();
            if valid["category"].is_null() {
                valid.as_object_mut().unwrap().remove("category");
            }
            if valid["meta"].is_null() {
                valid.as_object_mut().unwrap().remove("meta");
            }
            let entity = entity_entities::create(
                &ctx.db,
                fixture.workspace_id,
                entity_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: valid,
                },
                None,
            )
            .await
            .unwrap();
            if data["title"] == "present null" {
                use sea_orm::{ActiveModelTrait, ActiveValue::Set};
                entity_entities::ActiveModel {
                    id: Set(entity.id),
                    data: Set(data),
                    ..Default::default()
                }
                .update(&ctx.db)
                .await
                .unwrap();
            }
            entities.push(entity.id);
        }
        let mut fields = fields;
        fields["category"]["required"] = true.into();
        fields["meta"]["properties"]["a/b~c"]["required"] = true.into();
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            serde_json::from_value(
                serde_json::json!({"name":"note", "entity_types":{"note":{"fields":fields}}}),
            )
            .unwrap(),
            None,
            None,
        )
        .await
        .unwrap();
        entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title":"current", "category":"general"}),
            },
            None,
        )
        .await
        .unwrap();
        let drift = entity_entities::drift(&ctx.db, fixture.workspace_id, entities[0])
            .await
            .unwrap();
        assert_eq!(drift.schema_version, 1);
        assert_eq!(drift.active_schema_version, 2);
        assert!(drift.missing_fields.iter().all(|field| field.required));
        assert_eq!(
            drift
                .missing_fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            ["category", "/meta/a~1b~0c"]
        );
        for id in &entities[1..] {
            assert!(
                entity_entities::drift(&ctx.db, fixture.workspace_id, *id)
                    .await
                    .unwrap()
                    .missing_fields
                    .is_empty()
            );
        }
        let report = entity_entities::migration_dry_run(&ctx.db, fixture.workspace_id, "note")
            .await
            .unwrap();
        assert_eq!(report.active_version, 2);
        assert_eq!(report.by_entity_type.len(), 1);
        assert_eq!(report.by_entity_type[0].entity_type, "note");
        assert_eq!(report.by_entity_type[0].behind, 3);
        assert_eq!(report.by_entity_type[0].needs_values, 1);
        assert_eq!(
            (
                report.total_entities,
                report.current,
                report.behind_but_valid,
                report.needs_values
            ),
            (4, 1, 2, 1)
        );
        assert_eq!(
            report.by_entity_type[0].missing_required,
            ["/meta/a~1b~0c", "category"]
        );
    })
    .await;
}
