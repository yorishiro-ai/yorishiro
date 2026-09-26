//! OpenAPI documentation assembled from the routes and handler-local metadata.

use axum::Router;
use axum::http::Method;
use utoipa::openapi::extensions::Extensions;
use utoipa::openapi::path::{HttpMethod, PathItem};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, Info, OpenApiBuilder, PathsBuilder, RefOr, Schema};
use utoipa_swagger_ui::SwaggerUi;

use super::route_inventory::{Edition, RouteDoc, RouteEntry, RouteInventory};

fn openapi_method(method: &Method) -> HttpMethod {
    match *method {
        Method::GET => HttpMethod::Get,
        Method::POST => HttpMethod::Post,
        Method::PUT => HttpMethod::Put,
        Method::DELETE => HttpMethod::Delete,
        Method::PATCH => HttpMethod::Patch,
        Method::HEAD => HttpMethod::Head,
        Method::OPTIONS => HttpMethod::Options,
        Method::TRACE => HttpMethod::Trace,
        _ => panic!("unsupported OpenAPI method: {method}"),
    }
}

fn doc_for<'a>(docs: &'a [RouteDoc], entry: &RouteEntry) -> &'a RouteDoc {
    let method = openapi_method(&entry.method);
    docs.iter()
        .find(|doc| doc.path == entry.path && doc.methods.contains(&method))
        .unwrap_or_else(|| {
            panic!(
                "missing OpenAPI metadata for {} {}",
                entry.method, entry.path
            )
        })
}

fn add_schema(components: &mut Components, name: String, schema: RefOr<Schema>) {
    if let Some(existing) = components.schemas.get(&name) {
        if existing != &schema {
            panic!("conflicting OpenAPI component schema: {name}");
        }
        return;
    }
    components.schemas.insert(name, schema);
}

fn collect_refs(value: &serde_json::Value, refs: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(serde_json::Value::String(reference)) = object.get("$ref") {
                refs.push(reference.clone());
            }
            for value in object.values() {
                collect_refs(value, refs);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_refs(value, refs);
            }
        }
        _ => {}
    }
}

fn validate_schema_refs(
    operations: impl IntoIterator<Item = serde_json::Value>,
    components: &Components,
) {
    let mut values = operations.into_iter().collect::<Vec<_>>();
    values.extend(
        components
            .schemas
            .values()
            .map(|schema| serde_json::to_value(schema).expect("OpenAPI schema must serialize")),
    );

    let mut refs = Vec::new();
    for value in &values {
        collect_refs(value, &mut refs);
    }
    for reference in refs {
        let Some(name) = reference.strip_prefix("#/components/schemas/") else {
            continue;
        };
        assert!(
            components.schemas.contains_key(name),
            "missing OpenAPI component schema reference: {reference}"
        );
    }
}

/// Build the document for the operations in the runtime route inventory.
pub(crate) fn openapi_for(
    inventory: &RouteInventory,
    edition: Edition,
) -> utoipa::openapi::OpenApi {
    let mut paths = PathsBuilder::new();
    let mut components = Components::new();
    let mut operations = Vec::new();
    components.add_security_scheme(
        "bearer_auth",
        SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .bearer_format("yorishiro-api-key")
                .build(),
        ),
    );

    for entry in inventory.public(edition) {
        let doc = doc_for(&inventory.docs, entry);
        let method = openapi_method(&entry.method);
        let mut operation = doc.operation.clone();
        operation.operation_id = Some(entry.operation_id.clone());
        if entry.gated {
            operation
                .extensions
                .get_or_insert_with(Extensions::default)
                .insert(
                    "x-yorishiro-licence-required".into(),
                    serde_json::json!(true),
                );
        }
        operations
            .push(serde_json::to_value(&operation).expect("OpenAPI operation must serialize"));
        paths = paths.path(&entry.path, PathItem::new(method, operation));
        for (name, schema) in &doc.schemas {
            add_schema(&mut components, name.clone(), schema.clone());
        }
    }

    let mut error_schemas = Vec::new();
    <super::openapi::ApiErrorBody as utoipa::ToSchema>::schemas(&mut error_schemas);
    for (name, schema) in error_schemas {
        add_schema(&mut components, name, schema);
    }

    validate_schema_refs(operations, &components);

    OpenApiBuilder::new()
        .info(Info::new("Yorishiro REST API", env!("CARGO_PKG_VERSION")))
        .paths(paths.build())
        .components(Some(components))
        .build()
}

pub(crate) fn mount(router: Router, inventory: &RouteInventory) -> Router {
    let swagger = SwaggerUi::new("/docs").url(
        "/docs/openapi.json",
        openapi_for(inventory, Edition::Enterprise),
    );
    router.merge(swagger)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controllers::route_inventory::{RouteClass, RouteInventory};
    use axum::http::Method;

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
        let document =
            serde_json::to_value(openapi_for(&RouteInventory::default(), Edition::Enterprise))
                .unwrap();
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
            edition: Edition::Community,
            gated: false,
            operation_id: "get_health".into(),
            class: RouteClass::Infrastructure,
        });
        assert_eq!(inventory.public(Edition::Enterprise).count(), 0);
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
}
