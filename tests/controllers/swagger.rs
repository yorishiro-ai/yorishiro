use axum::http::Method;
use utoipa::openapi::{Components, RefOr, Schema};
use yorishiro::controllers::route_inventory::{RouteEntry, RouteInventory};

use yorishiro::controllers::route_inventory::RouteClass;
use yorishiro::controllers::swagger::implementation::*;

#[test]
fn openapi_method_covers_mounted_http_methods() {
    for method in [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::PATCH,
        Method::HEAD,
        Method::OPTIONS,
        Method::TRACE,
    ] {
        let _ = openapi_method(&method);
    }
}

#[test]
fn empty_inventory_still_has_the_bearer_scheme() {
    let document = serde_json::to_value(openapi_for(&RouteInventory::default())).unwrap();
    assert_eq!(
        document["components"]["securitySchemes"]["bearer_auth"]["scheme"],
        "bearer"
    );
}

#[test]
fn infrastructure_is_not_public() {
    let mut inventory = RouteInventory::default();
    inventory.entries.push(RouteEntry {
        path: "/_health".into(),
        method: Method::GET,
        operation_id: "get_health".into(),
        class: RouteClass::Infrastructure,
    });
    assert_eq!(inventory.public().count(), 0);
}

#[test]
#[should_panic(expected = "missing OpenAPI component schema reference")]
fn missing_schema_references_are_rejected() {
    validate_schema_refs(
        [serde_json::json!({
            "requestBody": {
                "content": {
                    "application/json": {
                        "schema": {
                            "properties": {
                                "nested": {
                                    "$ref": "#/components/schemas/Missing"
                                }
                            }
                        }
                    }
                }
            }
        })],
        &Components::new(),
    );
}

#[test]
#[should_panic(expected = "conflicting OpenAPI component schema")]
fn conflicting_duplicate_schemas_are_rejected() {
    let mut components = Components::new();
    add_schema(
        &mut components,
        "Duplicate".into(),
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .build()
            .into(),
    );
    add_schema(
        &mut components,
        "Duplicate".into(),
        utoipa::openapi::schema::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::Integer)
            .build()
            .into(),
    );
}

#[test]
fn identical_duplicate_schemas_are_allowed() {
    let mut components = Components::new();
    let schema: RefOr<Schema> = utoipa::openapi::Ref::from_schema_name("Shared").into();
    add_schema(&mut components, "Shared".into(), schema.clone());
    add_schema(&mut components, "Shared".into(), schema);
    assert_eq!(components.schemas.len(), 1);
}
