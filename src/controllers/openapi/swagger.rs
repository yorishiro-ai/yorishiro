//! `OpenAPI` documentation assembled from the routes and handler-local metadata.

pub mod implementation {

    use aide::openapi::OpenApi;
    use axum::{Json, Router, routing::get};

    use super::super::route_inventory::RouteInventory;
    use crate::models::iam::policy::{Action, Resource};

    /// Build the document for the operations in the runtime route inventory.
    ///
    /// # Panics
    /// Panics if route metadata or component schemas are inconsistent.
    #[must_use]
    pub fn openapi_for(inventory: &RouteInventory) -> OpenApi {
        let mut api = inventory.api.clone();
        let mut components = api.components.take().unwrap_or_default();
        for entry in inventory.public() {
            let path = api
                .paths
                .as_mut()
                .expect("generated paths")
                .paths
                .get_mut(&entry.path)
                .and_then(|path| path.as_item_mut())
                .expect("generated path");
            let target = match entry.method {
                axum::http::Method::GET => &mut path.get,
                axum::http::Method::POST => &mut path.post,
                axum::http::Method::PUT => &mut path.put,
                axum::http::Method::DELETE => &mut path.delete,
                axum::http::Method::PATCH => &mut path.patch,
                _ => panic!("unsupported API method"),
            };
            let operation = target.as_mut().expect("generated operation");
            operation.operation_id = Some(entry.operation_id.clone());
            let workspace = Resource::Workspace {
                tenant_id: uuid::Uuid::nil(),
                workspace_id: uuid::Uuid::nil(),
            };
            let workspace_operation = operation
                .extensions
                .get("x-yorishiro-required-actions")
                .unwrap_or(&serde_json::Value::Null)
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
                operation.parameters.push(aide::openapi::ReferenceOr::Item(
                    serde_json::from_value(serde_json::json!({"name": "X-Workspace-Id", "in": "header", "required": false, "description": description, "schema": {"type": "string", "format": "uuid"}})).expect("workspace header"),
                ));
                operation.responses.get_or_insert_with(Default::default).responses.entry(aide::openapi::StatusCode::Code(403)).or_insert_with(|| aide::openapi::ReferenceOr::Item(serde_json::from_value(serde_json::json!({"description": "The credential is not authorized for this operation or workspace.", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ApiErrorBody"}}}})).expect("authorization response")));
            }
        }
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
            paths: api.paths,
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

pub(crate) use implementation::mount;
