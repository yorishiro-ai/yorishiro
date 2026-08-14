use utoipa::OpenApi;

use super::*;

/// The hosted OpenAPI document is maintained by hand: adding a route to `router()` does not add
/// it to `HostedApiDoc`. This crate's endpoints were once absent from any spec at all, so the
/// document is checked against the routes actually served rather than trusted.
#[test]
fn every_route_this_crate_serves_appears_in_its_openapi_document() {
    let spec = HostedApiDoc::openapi();
    let documented: Vec<&str> = spec.paths.paths.keys().map(String::as_str).collect();

    for served in [
        "/hosted/stripe/webhook",
        "/hosted/tenant/overview",
        "/auth/oauth/authorize",
        "/auth/oauth/callback",
        "/auth/oauth/status",
    ] {
        assert!(
            documented.contains(&served),
            "{served} is served but missing from HostedApiDoc; documented: {documented:?}"
        );
    }
}

/// The spec is served to clients as JSON, so it has to serialise -- a schema referencing a type
/// that no longer derives `ToSchema` fails here rather than at request time.
#[test]
fn the_document_serialises() {
    let json = HostedApiDoc::openapi().to_json().unwrap();

    assert!(json.contains("/hosted/tenant/overview"));
}

/// `/api-docs/openapi.json` is the community edition's, mounted by `build_app`. This document is
/// a sibling and must not claim that path, since `Router::merge` panics on a duplicate route.
#[test]
fn the_document_does_not_claim_the_community_specs_path() {
    let spec = HostedApiDoc::openapi();

    assert!(!spec.paths.paths.contains_key("/api-docs/openapi.json"));
}
