//! The `OpenAPI` contract: route inventory, served documents and the schema shapes they name.

mod schemas;
pub use schemas::*;

pub mod swagger;

use aide::axum::{ApiRouter, routing::ApiMethodRouter};
use aide::openapi::OpenApi;
use axum::{body::Body, extract::Request, response::IntoResponse, routing::Route};
use loco_rs::app::AppContext;
use tower::{Layer, Service};

/// The standard `ApiErrorBody` response for each status.
pub(crate) fn error_responses(
    ctx: &mut aide::generate::GenContext,
    operation: &mut aide::openapi::Operation,
    statuses: &[u16],
) -> Vec<(Option<aide::openapi::StatusCode>, aide::openapi::Response)> {
    <axum::Json<ApiErrorBody> as aide::OperationOutput>::operation_response(ctx, operation)
        .into_iter()
        .flat_map(|response| {
            statuses.iter().map(move |status| {
                (
                    Some(aide::openapi::StatusCode::Code(*status)),
                    response.clone(),
                )
            })
        })
        .collect()
}

pub(crate) fn extension(
    name: &'static str,
    value: serde_json::Value,
) -> impl FnOnce(aide::transform::TransformOperation<'_>) -> aide::transform::TransformOperation<'_>
{
    move |mut operation| {
        operation.inner_mut().extensions.insert(name.into(), value);
        operation
    }
}

/// Keeps Loco's route groups and aide's inferred contract in one registration.
#[derive(Default)]
pub struct Routes {
    pub(crate) loco: loco_rs::controller::Routes,
    pub(crate) api: OpenApi,
    tag: Option<&'static str>,
}

impl Routes {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Tags every operation registered after this call.
    #[must_use]
    pub const fn tag(mut self, tag: &'static str) -> Self {
        self.tag = Some(tag);
        self
    }

    #[must_use]
    pub fn prefix(mut self, prefix: &str) -> Self {
        self.loco = self.loco.prefix(prefix);
        self
    }

    #[must_use]
    /// # Panics
    /// Panics if aide cannot assemble the registered operation.
    pub fn add(mut self, uri: &str, method: ApiMethodRouter<AppContext>) -> Self {
        let prefix = self
            .loco
            .prefix
            .as_deref()
            .unwrap_or_default()
            .trim_matches('/');
        let path = format!("/{prefix}/{}", uri.trim_matches('/'));
        let path = path.replace("//", "/");
        let path = if path == "/" {
            path.as_str()
        } else {
            path.trim_end_matches('/')
        };
        let mut api = OpenApi::default();
        let _ = ApiRouter::new()
            .api_route(path, method.clone())
            .finish_api(&mut api);
        for (path, method, operation) in operations_mut(&mut api) {
            operation.operation_id = Some(operation_id(method, path));
            operation.tags.extend(self.tag.map(Into::into));
        }
        merge_api(&mut self.api, api);
        self.loco = self.loco.add(uri, method.into());
        self
    }

    #[must_use]
    pub fn licensed(mut self) -> Self {
        for (_, _, operation) in operations_mut(&mut self.api) {
            operation
                .extensions
                .insert("x-yorishiro-licence-required".into(), true.into());
        }
        self
    }

    #[must_use]
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request<Body>> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request<Body>>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request<Body>>>::Error: Into<std::convert::Infallible> + 'static,
        <L::Service as Service<Request<Body>>>::Future: Send + 'static,
    {
        self.loco = self.loco.layer(layer);
        self
    }
}

pub(crate) fn merge_api(target: &mut OpenApi, source: OpenApi) {
    if let Some(paths) = source.paths {
        let target = &mut target.paths.get_or_insert_with(Default::default).paths;
        for (path, item) in paths.paths {
            let item = item.into_item().expect("generated path is inline");
            if let Some(existing) = target.get_mut(&path).and_then(|item| item.as_item_mut()) {
                for (method, _) in item.iter() {
                    assert!(
                        existing.iter().all(|(taken, _)| taken != method),
                        "duplicate route: {method} {path}"
                    );
                }
                existing.merge_with(item);
            } else {
                target.insert(path, aide::openapi::ReferenceOr::Item(item));
            }
        }
    }
    if let Some(components) = source.components {
        target
            .components
            .get_or_insert_with(Default::default)
            .schemas
            .extend(components.schemas);
    }
}

/// Every operation with its path and lowercase method.
pub(crate) fn operations_mut(
    api: &mut OpenApi,
) -> impl Iterator<Item = (&str, &'static str, &mut aide::openapi::Operation)> {
    api.paths
        .iter_mut()
        .flat_map(|paths| paths.paths.iter_mut())
        .filter_map(|(path, item)| Some((path.as_str(), item.as_item_mut()?)))
        .flat_map(|(path, item)| {
            [
                ("get", &mut item.get),
                ("post", &mut item.post),
                ("put", &mut item.put),
                ("delete", &mut item.delete),
                ("patch", &mut item.patch),
            ]
            .into_iter()
            .filter_map(move |(method, operation)| Some((path, method, operation.as_mut()?)))
        })
}

/// Stops the process at boot when two operations share an ID, since clients generate method names from it.
///
/// # Panics
/// Panics naming the colliding ID.
pub(crate) fn assert_unique_operation_ids(api: &mut OpenApi) {
    let mut seen = std::collections::HashSet::new();
    for (path, method, operation) in operations_mut(api) {
        let id = operation.operation_id.as_deref().unwrap_or_default();
        assert!(
            seen.insert(id.to_owned()),
            "duplicate operation id {id}: {method} {path}"
        );
    }
}

/// Stable, path-safe operation ID such as `get_api_entities_id`.
#[must_use]
pub fn operation_id(method: &str, path: &str) -> String {
    let suffix = path
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
        .to_ascii_lowercase();
    if suffix.is_empty() {
        method.to_owned()
    } else {
        format!("{method}_{suffix}")
    }
}
