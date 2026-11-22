//! `OpenAPI` documentation assembled from the routes and handler-local metadata.

#[cfg(not(feature = "openapi"))]
use super::route_inventory::RouteInventory;
#[cfg(not(feature = "openapi"))]
use axum::Router;
#[cfg(not(feature = "openapi"))]
pub(crate) const fn mount(router: Router, _inventory: &RouteInventory) -> Router {
    router
}

#[cfg(feature = "openapi")]
pub mod implementation {

    use aide::openapi::{Components, OpenApi, SchemaObject};
    use axum::{Json, Router, routing::get};

    use super::super::route_inventory::RouteInventory;
    use crate::models::iam::policy::{Action, Resource};

    /// # Panics
    /// Panics if two types use the same schema name with different definitions.
    pub fn add_schema(components: &mut Components, name: String, schema: SchemaObject) {
        if let Some(existing) = components.schemas.get(&name) {
            assert!(
                existing == &schema,
                "conflicting OpenAPI component schema: {name}"
            );
        } else {
            components.schemas.insert(name, schema);
        }
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

    ///
    /// # Panics
    /// Panics if an internal invariant required by this operation is violated.
    pub fn validate_schema_refs(
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
    ///
    /// # Panics
    /// Panics if route metadata or component schemas are inconsistent.
    #[must_use]
    pub fn openapi_for(inventory: &RouteInventory) -> OpenApi {
        let mut components = Components::default();
        let mut paths = serde_json::Map::new();
        let mut operations = Vec::new();
        for entry in inventory.public() {
            let doc = inventory
                .docs
                .iter()
                .find(|doc| doc.path == entry.path && doc.methods.contains(&entry.method))
                .expect("missing OpenAPI metadata");
            let mut operation =
                serde_json::to_value(&doc.operation).expect("OpenAPI operation must serialize");
            operation["operationId"] = entry.operation_id.clone().into();
            let workspace = Resource::Workspace {
                tenant_id: uuid::Uuid::nil(),
                workspace_id: uuid::Uuid::nil(),
            };
            let workspace_operation = operation["x-yorishiro-required-actions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| serde_json::from_value::<Action>(value.clone()).ok())
                .any(|action| action.applies_to(workspace));
            let key_creation =
                entry.path == "/api/api-keys" && entry.method == axum::http::Method::POST;
            if workspace_operation || key_creation {
                let description = if key_creation {
                    "Workspace binding for the new key. Do not send this header with tenant_scoped: true."
                } else {
                    "Explicit workspace selection. When omitted, operations that require one workspace may return 422 (`workspace_selection_required`) if several workspaces qualify."
                };
                let object = operation
                    .as_object_mut()
                    .expect("OpenAPI operation must be an object");
                let parameters = object
                    .entry("parameters")
                    .or_insert_with(|| serde_json::json!([]))
                    .as_array_mut()
                    .expect("parameters must be an array");
                if !parameters.iter().any(|parameter| {
                    parameter["in"] == "header"
                        && parameter["name"]
                            .as_str()
                            .is_some_and(|name| name.eq_ignore_ascii_case("X-Workspace-Id"))
                }) {
                    parameters.push(serde_json::json!({"name": "X-Workspace-Id", "in": "header", "required": false, "description": description, "schema": {"type": "string", "format": "uuid"}}));
                }
                operation["responses"].as_object_mut().expect("responses must be an object").entry("403").or_insert_with(|| serde_json::json!({"description": "The credential is not authorized for this operation or workspace.", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ApiErrorBody"}}}}));
            }
            paths
                .entry(entry.path.clone())
                .or_insert_with(|| serde_json::json!({}))
                [entry.method.as_str().to_ascii_lowercase()] = operation.clone();
            operations.push(operation);
            for (name, schema) in &doc.schemas {
                add_schema(&mut components, name.clone(), schema.clone());
            }
        }
        validate_schema_refs(operations, &components);
        components.security_schemes.insert(
            "bearer_auth".into(),
            serde_json::from_value(serde_json::json!({"type": "http", "scheme": "bearer", "bearerFormat": "yorishiro-api-key"}))
                .expect("bearer security scheme must deserialize"),
        );
        OpenApi {
            openapi: "3.1.0".into(),
            info: aide::openapi::Info {
                title: "Yorishiro REST API".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            paths: Some(
                serde_json::from_value(serde_json::Value::Object(paths))
                    .expect("OpenAPI paths must deserialize"),
            ),
            components: Some(components),
            ..Default::default()
        }
    }

    pub(crate) fn mount(router: Router, inventory: &RouteInventory) -> Router {
        let document = openapi_for(inventory);
        router
            .route(
                "/docs",
                get(aide::swagger::Swagger::new("/docs/openapi.json")
                    .with_title("Yorishiro REST API")
                    .axum_handler()),
            )
            .route(
                "/docs/openapi.json",
                get(move || async move { Json(document) }),
            )
    }
}

#[cfg(feature = "openapi")]
pub(crate) use implementation::mount;
