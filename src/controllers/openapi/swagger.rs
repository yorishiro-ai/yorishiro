//! `OpenAPI` documentation assembled from the routes and handler-local metadata.

pub mod implementation {

    use super::super::operations_mut;
    use aide::openapi::OpenApi;
    use axum::{Json, Router, routing::get};

    use crate::models::iam::policy::{Action, Resource};

    /// Completes the document the routes generated: the workspace header, the security scheme and the info block.
    ///
    /// # Panics
    /// Panics if static metadata is invalid.
    #[must_use]
    pub fn openapi_for(generated: &OpenApi) -> OpenApi {
        let mut api = generated.clone();
        let mut components = api.components.take().unwrap_or_default();
        let workspace = Resource::Workspace {
            tenant_id: uuid::Uuid::nil(),
            workspace_id: uuid::Uuid::nil(),
        };
        for (path, method, operation) in operations_mut(&mut api) {
            let workspace_operation = operation
                .operation_id
                .as_deref()
                .and_then(Action::from_name)
                .is_some_and(|action| action.applies_to(workspace));
            let key_creation = path == "/api/access-keys" && method == "post";
            if workspace_operation || key_creation {
                let description = if key_creation {
                    "Workspace binding for the new key. Do not send this header with tenant_scoped: true."
                } else {
                    "Explicit workspace selection. When omitted, operations that require one workspace may return 422 (`workspace_selection_required`) if several workspaces qualify."
                };
                operation.parameters.push(aide::openapi::ReferenceOr::Item(
                    serde_json::from_value(serde_json::json!({"name": "x-workspace-id", "in": "header", "required": false, "description": description, "schema": {"type": "string", "format": "uuid"}})).expect("workspace header"),
                ));
            }
        }
        components.security_schemes.insert(
            "bearer_auth".into(),
            serde_json::from_value(serde_json::json!({"type": "http", "scheme": "bearer", "bearerFormat": "yorishiro-access-key"}))
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

    pub(crate) fn mount(router: Router, generated: &OpenApi) -> Router {
        let document = openapi_for(generated);
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
