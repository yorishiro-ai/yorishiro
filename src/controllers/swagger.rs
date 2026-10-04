//! OpenAPI documentation assembled from the routes and handler-local metadata.

#[cfg(not(feature = "openapi"))]
use super::route_inventory::RouteInventory;
#[cfg(not(feature = "openapi"))]
use axum::Router;
#[cfg(not(feature = "openapi"))]
pub(crate) fn mount(router: Router, _inventory: &RouteInventory) -> Router {
    router
}

#[cfg(feature = "openapi")]
pub mod implementation {

    use axum::Router;
    use axum::http::Method;
    use utoipa::openapi::path::{HttpMethod, PathItem};
    use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
    use utoipa::openapi::{Components, Info, OpenApiBuilder, PathsBuilder, RefOr, Schema};
    use utoipa_swagger_ui::SwaggerUi;

    use super::super::route_inventory::{RouteDoc, RouteEntry, RouteInventory};

    ///
    /// # Panics
    /// Panics if an internal invariant required by this operation is violated.
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
            operations
                .push(serde_json::to_value(&operation).expect("OpenAPI operation must serialize"));
            paths = paths.path(&entry.path, PathItem::new(method, operation));
            for (name, schema) in &doc.schemas {
                add_schema(&mut components, name.clone(), schema.clone());
            }
        }

        let mut error_schemas = Vec::new();
        <super::super::openapi::ApiErrorBody as utoipa::ToSchema>::schemas(&mut error_schemas);
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
