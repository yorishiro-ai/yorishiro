use axum::http::StatusCode;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::data::templates::{TEMPLATES, parse};
use yorishiro::models::api_keys::ApiKeyScope;

use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant, issue_api_key};

#[tokio::test]
async fn every_builtin_relation_type_can_be_created() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            workspace_id,
            owner_id,
            api_key: schema_key,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let write_key =
            issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Write, false).await;

        for template in TEMPLATES {
            let definition = parse(template);
            let schema_name = definition.name.clone();
            let schema = request
                .post("/api/schemas")
                .add_header("Authorization", format!("Bearer {schema_key}"))
                .json(&serde_json::to_value(&definition).unwrap())
                .await;
            assert_eq!(
                schema.status_code(),
                StatusCode::CREATED,
                "{}: {}",
                template.id,
                schema.text()
            );

            let mut ids = std::collections::HashMap::new();
            for (entity_type, entity) in &definition.entity_types {
                let data = required_data(&entity.fields);
                let response = request
                    .post("/api/entities")
                    .add_header("Authorization", format!("Bearer {write_key}"))
                    .json(&serde_json::json!({
                        "schema_name": schema_name,
                        "entity_type": entity_type,
                        "data": data,
                    }))
                    .await;
                assert_eq!(
                    response.status_code(),
                    StatusCode::CREATED,
                    "{} {}: {}",
                    template.id,
                    entity_type,
                    response.text()
                );
                ids.insert(
                    entity_type.clone(),
                    response.json::<serde_json::Value>()["id"]
                        .as_str()
                        .unwrap()
                        .parse::<Uuid>()
                        .unwrap(),
                );
            }

            for (relation_type, relation) in &definition.relation_types {
                let target_id = if relation.source == relation.target {
                    let entity = &definition.entity_types[&relation.target];
                    let response = request
                        .post("/api/entities")
                        .add_header("Authorization", format!("Bearer {write_key}"))
                        .json(&serde_json::json!({
                            "schema_name": schema_name,
                            "entity_type": relation.target,
                            "data": required_data(&entity.fields),
                        }))
                        .await;
                    assert_eq!(
                        response.status_code(),
                        StatusCode::CREATED,
                        "{} {} target: {}",
                        template.id,
                        relation_type,
                        response.text()
                    );
                    response.json::<serde_json::Value>()["id"]
                        .as_str()
                        .unwrap()
                        .parse::<Uuid>()
                        .unwrap()
                } else {
                    ids[&relation.target]
                };
                let response = request
                    .post("/api/relations")
                    .add_header("Authorization", format!("Bearer {write_key}"))
                    .json(&serde_json::json!({
                        "source_id": ids[&relation.source],
                        "target_id": target_id,
                        "relation_type": relation_type,
                        "properties": {},
                    }))
                    .await;
                assert_eq!(
                    response.status_code(),
                    StatusCode::CREATED,
                    "{} {}: {}",
                    template.id,
                    relation_type,
                    response.text()
                );
            }
        }
    })
    .await;
}

fn required_data(
    fields: &std::collections::BTreeMap<
        String,
        yorishiro::models::schema_schemas::metaschema::FieldDef,
    >,
) -> serde_json::Value {
    let mut data = serde_json::Map::new();
    for (name, field) in fields {
        if field.required {
            data.insert(
                name.clone(),
                match field.r#type {
                    yorishiro::models::schema_schemas::metaschema::FieldTypeName::String => {
                        match field.format.as_deref() {
                            Some("date") => serde_json::json!("2026-01-01"),
                            Some("date-time") => serde_json::json!("2026-01-01T00:00:00Z"),
                            Some("uri") => serde_json::json!("https://example.com"),
                            Some("email") => serde_json::json!("test@example.com"),
                            _ if field.pattern.is_some() => {
                                serde_json::json!("0000-0000-0000-000X")
                            }
                            _ => field
                                .enum_values
                                .as_ref()
                                .and_then(|values| values.first())
                                .map_or_else(
                                    || serde_json::json!("test"),
                                    |value| serde_json::json!(value),
                                ),
                        }
                    }
                    yorishiro::models::schema_schemas::metaschema::FieldTypeName::Integer => {
                        serde_json::json!(1)
                    }
                    yorishiro::models::schema_schemas::metaschema::FieldTypeName::Number => {
                        serde_json::json!(1)
                    }
                    yorishiro::models::schema_schemas::metaschema::FieldTypeName::Boolean => {
                        serde_json::json!(true)
                    }
                    yorishiro::models::schema_schemas::metaschema::FieldTypeName::Array => {
                        serde_json::json!([])
                    }
                    yorishiro::models::schema_schemas::metaschema::FieldTypeName::Object => {
                        serde_json::json!({})
                    }
                },
            );
        }
    }
    serde_json::Value::Object(data)
}
