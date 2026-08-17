//! The paid edition: Stripe billing, OAuth/OIDC, the marketplace, LLM-backed fill, and the SPA.
//!
//! This crate's binary is the only one the product ships, so a self-hosted deployment runs it
//! too -- what a deployment gets is decided by its licence key and its configuration, not by
//! which binary it installed. The separation that matters is the dependency direction:
//! `yorishiro-core` and `yorishiro-server` do not depend on this crate, and composing them here
//! is what lets the paid half be removed without touching them.

pub mod error;
pub mod http;
pub mod repositories;
pub mod services;
pub mod state;
pub mod web;

use std::sync::LazyLock;

use axum::Router;
use axum::routing::{get, post, put};
use utoipa::OpenApi;

use http::controllers::{HostedApiDoc, dashboard, inference, marketplace, oauth, origin, stripe};
use state::HostedState;

/// The hosted dashboard/webhook/OAuth-status router, mounted by `yorishiro-server`'s
/// `main` (or, alternatively, nested into `yorishiro-server`'s own router by a deployment that
/// prefers a single process). `/auth/oauth/status` always returns `200` regardless of whether
/// OAuth is configured, so the Web UI's login page can tell "OAuth not configured" apart from
/// "this deployment predates OAuth entirely" -- see [`oauth_login_router`] for
/// `/auth/oauth/authorize|callback`, which behave differently and are mounted separately.
///
/// Reachable without a bearer token exactly like `yorishiro-server`'s own `/auth/*`/`/setup*`
/// routes, but split into two builders (this one, and [`oauth_login_router`]) rather than
/// one flat `Router` because they need different treatment: `/auth/oauth/authorize|callback` are
/// as brute-forceable as a login form and need the same rate limiter `yorishiro-server` applies
/// to its own unauthenticated routes; everything else here either requires a bearer token
/// already (`/hosted/tenant/overview`), is a Stripe-signature-verified webhook that must never
/// be rate-limited (dropping a legitimate billing event on `429` is worse than not rate-limiting
/// it), or is `/auth/oauth/status`, deliberately unlimited because the Web UI's login page polls
/// it on every load. `apply_rate_limit_layer` itself lives in `yorishiro-server`, and a layer
/// can only be applied where the routers are composed -- which is `yorishiro-server`'s `main`,
/// since that is what merges these two sub-routers with the community edition's own. Applying
/// it here instead would give the OAuth routes a second `RateLimiter`, so the same client would
/// get two independent quotas rather than the one shared with `/auth/login`.
pub fn router() -> Router<HostedState> {
    Router::new()
        .route("/hosted/stripe/webhook", post(stripe::stripe_webhook))
        .route("/hosted/tenant/overview", get(dashboard::tenant_overview))
        .route("/auth/oauth/status", get(oauth::status))
        .route("/api-docs/hosted-openapi.json", get(hosted_openapi))
        // The marketplace is an enterprise capability, so it lives here: the community edition
        // serves none of these paths, so nothing is being shadowed. Each path still declares
        // **every** method it needs in one `.route`, because a path defined on this router takes
        // that path entirely: a method left out would answer 405 rather than falling through.
        .route("/api/marketplace", get(marketplace::list_marketplace))
        .route(
            "/api/marketplace/{id}/versions",
            get(marketplace::list_versions).post(marketplace::publish_version),
        )
        .route(
            "/api/marketplace/{id}/reviews",
            get(marketplace::list_reviews).post(marketplace::submit_review),
        )
        .route(
            "/api/marketplace/{id}/fork",
            post(marketplace::fork_template),
        )
        .route(
            "/api/marketplace/{id}/visibility",
            put(marketplace::set_visibility),
        )
        // The origin/merge chain is also an enterprise capability. These paths overlay base's
        // `/api/schemas` namespace, so the shadowing rule matters here: a path defined on this
        // router takes that path entirely. `merge-preview` and `merge` are distinct trailing
        // segments, so base's `/api/schemas/{schema_id}` is untouched. `/api/schemas/upstream-
        // changes` is the one to watch: base has no such literal path, and its `{schema_id}`
        // route would otherwise catch the word as a UUID and answer 400; this router takes it
        // first.
        .route(
            "/api/schemas/upstream-changes",
            get(origin::list_upstream_changes),
        )
        .route(
            "/api/schemas/{schema_id}/merge-preview",
            get(origin::merge_preview),
        )
        .route("/api/schemas/{schema_id}/merge", post(origin::merge_apply))
        // Fill mode B is an enterprise capability for the same reason as the rest: the server
        // makes an outbound chat completion, and a bring-your-own-key design moves who pays for
        // it without changing that.
        //
        // The shadowing rule applies again, across two namespaces. `infer-fill` is a distinct
        // trailing segment under `/api/schemas/active/{name}`, so base's own routes there are
        // untouched. `proposals` and `confirm` sit beside base's surviving `undo` on the
        // `/api/migration-jobs/{job_id}` prefix -- distinct trailing segments again, so the
        // three coexist and confirming still snapshots through the mechanism `undo` reverses.
        .route(
            "/api/workspace/llm-key",
            put(inference::set_llm_key)
                .get(inference::get_llm_key)
                .delete(inference::delete_llm_key),
        )
        .route(
            "/api/schemas/active/{name}/infer-fill",
            post(inference::infer_fill),
        )
        .route(
            "/api/migration-jobs/{job_id}/proposals",
            get(inference::list_proposals),
        )
        .route(
            "/api/migration-jobs/{job_id}/confirm",
            post(inference::confirm_proposals),
        )
}

/// Serialized once on first request rather than per call -- the document is fixed at compile
/// time, and `to_json` walks the whole structure.
static HOSTED_OPENAPI_JSON: LazyLock<String> = LazyLock::new(|| {
    HostedApiDoc::openapi()
        .to_json()
        .expect("the derived OpenAPI document is static and always serializable")
});

/// `GET /api-docs/hosted-openapi.json` -- the OpenAPI document for this crate's own routes.
///
/// A sibling of the community edition's `/api-docs/openapi.json` rather than an addition to it:
/// `build_app` mounts that route itself from a `pub(crate)` `ApiDoc` this crate cannot reach,
/// and `Router::merge` panics on a duplicate path. See [`http::controllers::HostedApiDoc`].
///
/// Unauthenticated, matching how the community edition serves its own spec, and not
/// rate-limited -- it is a static document containing no tenant data.
async fn hosted_openapi() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        HOSTED_OPENAPI_JSON.as_str(),
    )
}

/// `/auth/oauth/authorize`/`/auth/oauth/callback` -- the two routes in this crate that need the
/// same brute-force protection `yorishiro-server` applies to its own `/auth/login`/`/auth/signup`.
/// Both are always mounted here -- they 404 internally (see `http::controllers::oauth`) rather
/// than being conditionally added, so their presence/absence never depends on route-table state,
/// only on the request they each handle. See [`router`]'s doc comment for why these two are
/// split into their own builder instead of living there.
pub fn oauth_login_router() -> Router<HostedState> {
    Router::new()
        .route("/auth/oauth/authorize", get(oauth::authorize))
        .route("/auth/oauth/callback", get(oauth::callback))
}

#[cfg(test)]
#[path = "../tests/lib.rs"]
mod tests;
