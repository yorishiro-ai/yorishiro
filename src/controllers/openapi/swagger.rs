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

    use axum::Router;
    use axum::http::Method;
    use utoipa::openapi::path::{HttpMethod, Operation, ParameterBuilder, ParameterIn, PathItem};
    use utoipa::openapi::response::ResponseBuilder;
    use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type};
    use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
    use utoipa::openapi::{
        Components, Content, Info, OpenApiBuilder, PathsBuilder, Ref, RefOr, Required, Schema,
    };
    use utoipa_swagger_ui::SwaggerUi;

    use super::super::route_inventory::{RouteDoc, RouteEntry, RouteInventory};
    use crate::models::iam::policy::{Action, Resource};

    fn document_workspace_selection(entry: &RouteEntry, operation: &mut Operation) {
        let workspace = Resource::Workspace {
            tenant_id: uuid::Uuid::nil(),
            workspace_id: uuid::Uuid::nil(),
        };
        let workspace_operation = operation
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("x-yorishiro-required-actions"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| serde_json::from_value::<Action>(value.clone()).ok())
            .any(|action| action.applies_to(workspace));
        let key_creation = entry.path == "/api/api-keys" && entry.method == Method::POST;
        if !workspace_operation && !key_creation {
            return;
        }
        let name = "X-Workspace-Id";
        let parameters = operation.parameters.get_or_insert_with(Vec::new);
        if !parameters.iter().any(|parameter| {
            matches!(parameter, RefOr::T(parameter) if parameter.parameter_in == ParameterIn::Header
                && parameter.name.eq_ignore_ascii_case(name))
        }) {
            let description = if key_creation {
                "Workspace binding for the new key. Do not send this header with tenant_scoped: true."
            } else {
                "Explicit workspace selection. When omitted, operations that require one workspace may return 422 (`workspace_selection_required`) if several workspaces qualify."
            };
            parameters.push(
                ParameterBuilder::new()
                    .name(name)
                    .parameter_in(ParameterIn::Header)
                    .required(Required::False)
                    .schema(Some(
                        ObjectBuilder::new()
                            .schema_type(Type::String)
                            .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
                            .build(),
                    ))
                    .description(Some(description))
                    .build()
                    .into(),
            );
        }
        operation
            .responses
            .responses
            .entry("403".into())
            .or_insert_with(|| {
                ResponseBuilder::new()
                    .description(
                        "The credential is not authorized for this operation or workspace.",
                    )
                    .content(
                        "application/json",
                        Content::new(Some(Ref::from_schema_name("ApiErrorBody"))),
                    )
                    .build()
                    .into()
            });
    }

    ///
    /// # Panics
    /// Panics if an internal invariant required by this operation is violated.
    #[must_use]
    pub fn openapi_method(method: &Method) -> HttpMethod {
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

    ///
    /// # Panics
    /// Panics if an internal invariant required by this operation is violated.
    pub fn add_schema(components: &mut Components, name: String, schema: RefOr<Schema>) {
        if let Some(existing) = components.schemas.get(&name) {
            assert!(
                existing == &schema,
                "conflicting OpenAPI component schema: {name}"
            );
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
    /// Panics if an internal invariant required by this operation is violated.
    #[must_use]
    pub fn openapi_for(inventory: &RouteInventory) -> utoipa::openapi::OpenApi {
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

        for entry in inventory.public() {
            let doc = doc_for(&inventory.docs, entry);
            let method = openapi_method(&entry.method);
            let mut operation = doc.operation.clone();
            operation.operation_id = Some(entry.operation_id.clone());
            document_workspace_selection(entry, &mut operation);
            operations
                .push(serde_json::to_value(&operation).expect("OpenAPI operation must serialize"));
            paths = paths.path(&entry.path, PathItem::new(method, operation));
            for (name, schema) in &doc.schemas {
                add_schema(&mut components, name.clone(), schema.clone());
            }
        }

        let mut error_schemas = Vec::new();
        <super::super::ApiErrorBody as utoipa::ToSchema>::schemas(&mut error_schemas);
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
        let swagger = SwaggerUi::new("/docs").url("/docs/openapi.json", openapi_for(inventory));
        router.merge(swagger)
    }
}

#[cfg(feature = "openapi")]
pub(crate) use implementation::mount;
