use axum::http::Method;
use loco_rs::controller::AppRoutes;
#[cfg(feature = "openapi")]
use utoipa::openapi::path::{HttpMethod, Operation};
#[cfg(feature = "openapi")]
use utoipa::openapi::{RefOr, Schema};

/// Whether a route is part of the public REST contract or an implementation endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteClass {
    Public,
    #[cfg(feature = "openapi")]
    Infrastructure,
}

/// One operation expanded from a mounted Loco route group.
#[derive(Clone, Debug)]
pub struct RouteEntry {
    pub path: String,
    pub method: Method,
    pub operation_id: String,
    #[cfg(feature = "openapi")]
    pub class: RouteClass,
}

/// The route inventory produced while the application route tree is composed.
#[derive(Clone, Default)]
pub struct RouteInventory {
    pub entries: Vec<RouteEntry>,
    pub exclusions: Vec<InfrastructureExclusion>,
    #[cfg(feature = "openapi")]
    pub(crate) docs: Vec<RouteDoc>,
}

/// OpenAPI metadata emitted next to the handler that serves an operation.
#[cfg(feature = "openapi")]
#[derive(Clone)]
pub struct RouteDoc {
    pub(crate) path: String,
    pub(crate) methods: Vec<HttpMethod>,
    pub(crate) operation: Operation,
    pub(crate) schemas: Vec<(String, RefOr<Schema>)>,
}

#[cfg(feature = "openapi")]
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

#[cfg(feature = "openapi")]
pub(crate) fn path_doc<P>(_: P) -> RouteDoc
where
    P: utoipa::Path + utoipa::__dev::SchemaReferences,
{
    let mut schemas = Vec::new();
    <P as utoipa::__dev::SchemaReferences>::schemas(&mut schemas);
    RouteDoc {
        path: P::path(),
        methods: P::methods(),
        operation: P::operation(),
        schemas,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InfrastructureExclusion {
    pub path: &'static str,
    pub reason: &'static str,
}

impl RouteInventory {
    pub(crate) fn add_group(&mut self, routes: &AppRoutes, class: RouteClass) {
        #[cfg(not(feature = "openapi"))]
        let _ = class;
        for route in routes.collect() {
            for method in route.actions {
                let operation_id = operation_id(&method, &route.uri);
                self.entries.push(RouteEntry {
                    path: route.uri.clone(),
                    method,
                    operation_id,
                    #[cfg(feature = "openapi")]
                    class,
                });
            }
        }
    }

    #[cfg(feature = "openapi")]
    pub(crate) fn add_docs(&mut self, docs: impl IntoIterator<Item = RouteDoc>) {
        self.docs.extend(docs);
    }

    pub(crate) fn add_infrastructure(&mut self, routes: &AppRoutes) {
        for route in routes.collect() {
            infrastructure_reason(&route.uri)
                .unwrap_or_else(|| panic!("unallowlisted infrastructure route: {}", route.uri));
            for method in route.actions {
                self.entries.push(RouteEntry {
                    path: route.uri.clone(),
                    method: method.clone(),
                    operation_id: operation_id(&method, &route.uri),
                    #[cfg(feature = "openapi")]
                    class: RouteClass::Infrastructure,
                });
            }
        }
    }

    #[cfg(feature = "openapi")]
    pub fn public(&self) -> impl Iterator<Item = &RouteEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.class == RouteClass::Public)
    }

    pub(crate) fn validate(&self) {
        let mut operation_ids = std::collections::HashSet::new();
        let mut operations = std::collections::HashSet::new();
        for entry in &self.entries {
            assert!(
                operations.insert((entry.path.clone(), entry.method.clone())),
                "duplicate composed route: {} {}",
                entry.method,
                entry.path
            );
            assert!(
                operation_ids.insert(entry.operation_id.clone()),
                "duplicate operation id: {}",
                entry.operation_id
            );
        }
        for exclusion in &self.exclusions {
            assert!(
                !self
                    .entries
                    .iter()
                    .any(|entry| entry.path == exclusion.path)
            );
        }

        #[cfg(feature = "openapi")]
        {
            let mut docs = std::collections::HashSet::new();
            for doc in &self.docs {
                for method in &doc.methods {
                    assert!(
                        docs.insert((doc.path.clone(), method.clone())),
                        "duplicate OpenAPI metadata: {}",
                        doc.path,
                    );
                }
            }
            for entry in self.public() {
                assert!(
                    self.docs.iter().any(|doc| {
                        doc.path == entry.path
                            && doc.methods.contains(&openapi_method(&entry.method))
                    }),
                    "missing OpenAPI metadata for {} {}",
                    entry.method,
                    entry.path
                );
            }
            for doc in &self.docs {
                assert!(
                    self.public().any(|entry| {
                        doc.path == entry.path
                            && doc.methods.contains(&openapi_method(&entry.method))
                    }),
                    "OpenAPI metadata has no composed route: {}",
                    doc.path
                );
            }
        }
    }

    pub fn add_allowlisted_exclusions(&mut self) {
        self.exclusions = vec![
            InfrastructureExclusion {
                path: "/docs",
                reason: "Swagger UI asset route, not a product API operation",
            },
            InfrastructureExclusion {
                path: "/docs/{*wildcard}",
                reason: "Swagger UI asset route, not a product API operation",
            },
            InfrastructureExclusion {
                path: "/docs/openapi.json",
                reason: "Generated contract document, not a product API operation",
            },
            InfrastructureExclusion {
                path: "/mcp",
                reason: "MCP protocol transport, documented by the MCP service",
            },
        ];
    }
}

pub fn operation_id(method: &Method, path: &str) -> String {
    let suffix = path
        .trim_matches('/')
        .replace(['/', '{', '}', '-', '.'], "_")
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
        .to_ascii_lowercase();
    let method = method.as_str().to_ascii_lowercase();
    if suffix.is_empty() {
        method
    } else {
        format!("{method}_{suffix}")
    }
}

fn infrastructure_reason(path: &str) -> Option<&'static str> {
    match path {
        "/_ping" => Some("Loco liveness probe, not a product API operation"),
        "/_health" => Some("Loco health probe, not a product API operation"),
        "/_readiness" => Some("Loco readiness probe, not a product API operation"),
        _ => None,
    }
}
