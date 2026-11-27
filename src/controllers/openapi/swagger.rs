//! Serves the generated `OpenAPI` document and Swagger UI.

use aide::{
    openapi::{Info, OpenApi, Parameter, ReferenceOr, SecurityScheme},
    transform::TransformOpenApi,
};
use axum::{Json, Router, routing::get};

use crate::models::iam::policy::{Action, Resource};

pub(crate) fn describe(mut transform: TransformOpenApi<'_>) -> TransformOpenApi<'_> {
    let api = transform.inner_mut();
    api.openapi = "3.1.0".into();
    let workspace = Resource::Workspace {
        tenant_id: uuid::Uuid::nil(),
        workspace_id: uuid::Uuid::nil(),
    };
    let workspace_header: Parameter = serde_json::from_value(serde_json::json!({
        "name": crate::models::access_keys::WORKSPACE_HEADER,
        "in": "header",
        "description": "Explicit workspace selection. When omitted, operations that require one workspace may return 422 (`workspace_selection_required`) if several workspaces qualify.",
        "schema": {"type": "string", "format": "uuid"}
    }))
    .expect("workspace header parameter is valid");
    for (_, _, operation) in super::operations_mut(api) {
        if operation
            .operation_id
            .as_deref()
            .and_then(Action::from_name)
            .is_some_and(|action| action.applies_to(workspace))
        {
            operation
                .parameters
                .push(ReferenceOr::Item(workspace_header.clone()));
        }
    }
    transform
        .info(Info {
            title: "Yorishiro REST API".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            ..Default::default()
        })
        .security_scheme(
            "bearer_auth",
            SecurityScheme::Http {
                scheme: "bearer".into(),
                bearer_format: Some("yorishiro-access-key".into()),
                description: None,
                extensions: std::iter::empty().collect(),
            },
        )
}

pub(crate) fn mount(router: Router, document: OpenApi) -> Router {
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
