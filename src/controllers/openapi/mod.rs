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

/// Keeps Loco's route groups and aide's inferred contract in one registration.
pub struct Routes {
    pub(crate) loco: loco_rs::controller::Routes,
    pub(crate) api: ApiRouter<AppContext>,
    pub(crate) action_paths: Vec<(String, String)>,
    tag: Option<&'static str>,
    licensed: bool,
}

impl Default for Routes {
    fn default() -> Self {
        Self {
            loco: loco_rs::controller::Routes::new(),
            api: ApiRouter::new(),
            action_paths: Vec::new(),
            tag: None,
            licensed: false,
        }
    }
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
    pub fn add(self, uri: &str, method: ApiMethodRouter<AppContext>) -> Self {
        self.add_action(uri, "", method)
    }

    /// Documents a command suffix that the handler parses because Axum captures the entire path segment.
    #[must_use]
    pub(crate) fn add_action(
        mut self,
        uri: &str,
        suffix: &str,
        method: ApiMethodRouter<AppContext>,
    ) -> Self {
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
        if !suffix.is_empty() {
            self.action_paths
                .push((path.to_owned(), format!("{path}{suffix}")));
        }
        self.api = self.api.api_route_with(path, method.clone(), |mut item| {
            if let Some(tag) = self.tag {
                item = item.tag(tag);
            }
            if self.licensed {
                for operation in
                    path_operations_mut(item.inner_mut()).map(|(_, operation)| operation)
                {
                    operation
                        .extensions
                        .insert("x-yorishiro-licence-required".into(), true.into());
                }
            }
            item
        });
        self.loco = self.loco.add(uri, method.into());
        self
    }

    /// Marks subsequently registered operations as requiring a licence.
    #[must_use]
    pub const fn licensed(mut self) -> Self {
        self.licensed = true;
        self
    }

    /// Returns the licence requirement shared by route mounting and documentation.
    #[must_use]
    pub const fn requires_licence(&self) -> bool {
        self.licensed
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

pub(crate) fn document_action_paths(api: &mut OpenApi, action_paths: Vec<(String, String)>) {
    let Some(paths) = &mut api.paths else { return };
    for (path, action_path) in action_paths {
        let item = paths
            .paths
            .get_mut(&path)
            .and_then(|item| item.as_item_mut())
            .expect("action route is registered");
        let post = item.post.take().expect("action routes use POST");
        if item.iter().next().is_none() {
            paths.paths.shift_remove(&path);
        }
        let previous = paths.paths.insert(
            action_path,
            aide::openapi::ReferenceOr::Item(aide::openapi::PathItem {
                post: Some(post),
                ..Default::default()
            }),
        );
        assert!(previous.is_none(), "duplicate action path");
    }
}

fn path_operations_mut(
    item: &mut aide::openapi::PathItem,
) -> impl Iterator<Item = (&'static str, &mut aide::openapi::Operation)> {
    [
        ("get", &mut item.get),
        ("post", &mut item.post),
        ("put", &mut item.put),
        ("delete", &mut item.delete),
        ("patch", &mut item.patch),
        ("head", &mut item.head),
        ("options", &mut item.options),
        ("trace", &mut item.trace),
    ]
    .into_iter()
    .filter_map(|(method, operation)| Some((method, operation.as_mut()?)))
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
            path_operations_mut(item).map(move |(method, operation)| (path, method, operation))
        })
}

/// Stops the process at boot when two operations share an ID, since clients generate method names from it.
///
/// # Panics
/// Panics naming the colliding ID.
pub(crate) fn finalize_operation_ids(api: &mut OpenApi) {
    let mut seen = std::collections::HashSet::new();
    for (path, method, operation) in operations_mut(api) {
        let id = operation
            .operation_id
            .get_or_insert_with(|| operation_id(method, path));
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
