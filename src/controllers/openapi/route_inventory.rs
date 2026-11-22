//! Inventory of every mounted route, built while the route tree is composed.
//!
//! It checks that each public route has `OpenAPI` documentation and that anything left undocumented is an explicitly allowed exclusion.

use axum::http::Method;
use loco_rs::controller::AppRoutes;

/// Whether a route is part of the public REST contract or an implementation endpoint.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum RouteClass {
    Public,
    Infrastructure,
}

/// One operation expanded from a mounted Loco route group.
#[derive(Clone)]
pub struct RouteEntry {
    pub path: String,
    pub method: Method,
    pub operation_id: String,
    pub class: RouteClass,
}

/// The route inventory produced while the application route tree is composed.
#[derive(Clone, Default)]
pub struct RouteInventory {
    pub entries: Vec<RouteEntry>,
    pub exclusions: Vec<InfrastructureExclusion>,
    pub(crate) api: aide::openapi::OpenApi,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct InfrastructureExclusion {
    pub path: &'static str,
    pub reason: &'static str,
}

impl RouteInventory {
    pub(crate) fn add_group(&mut self, routes: &AppRoutes, class: RouteClass) {
        for route in routes.collect() {
            for method in route.actions {
                let operation_id = operation_id(&method, &route.uri);
                self.entries.push(RouteEntry {
                    path: route.uri.clone(),
                    method,
                    operation_id,
                    class,
                });
            }
        }
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
                    class: RouteClass::Infrastructure,
                });
            }
        }
    }

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

        for entry in self.public() {
            assert!(
                self.api
                    .paths
                    .as_ref()
                    .and_then(|paths| paths.paths.get(&entry.path))
                    .and_then(|item| item.as_item())
                    .is_some_and(|path| path
                        .iter()
                        .any(|(method, _)| method == entry.method.as_str().to_ascii_lowercase())),
                "missing OpenAPI metadata for {} {}",
                entry.method,
                entry.path
            );
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

#[must_use]
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
