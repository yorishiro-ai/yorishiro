pub mod dashboard;
pub mod entity_columns;
pub mod inference;
pub mod marketplace;
pub mod oauth;
pub mod origin;
pub mod stripe;

use utoipa::OpenApi;

/// OpenAPI document for the routes *this* repo adds.
/// Deliberately separate from the embedded community server's document rather than merged into it: that one is built by `yorishiro_server`'s own `ApiDoc`, which lives behind a `pub(crate) mod controllers` and so cannot be reached, extended, or re-served from here.
/// `build_app` also already owns the `/api-docs/openapi.json` route, and `axum::Router::merge` panics on a duplicate path, so a single combined document is not something this side can produce.
///
/// The result is two specs from one process, each canonical for its own half: the community API at `/api-docs/openapi.json`, and this crate's own routes at `/api-docs/hosted-openapi.json`.
/// Unifying them needs a `build_app` variant upstream that accepts an extra `OpenApi`.
#[derive(OpenApi)]
#[openapi(
    paths(
        stripe::stripe_webhook,
        dashboard::tenant_overview,
        oauth::status,
        oauth::authorize,
        oauth::callback,
        marketplace::list_marketplace,
        marketplace::list_versions,
        marketplace::publish_version,
        marketplace::list_reviews,
        marketplace::submit_review,
        marketplace::fork_template,
        marketplace::set_visibility,
        origin::list_upstream_changes,
        origin::merge_preview,
        origin::merge_apply,
        inference::set_llm_key,
        inference::get_llm_key,
        inference::delete_llm_key,
        inference::infer_fill,
        inference::list_proposals,
        inference::confirm_proposals,
        entity_columns::list_columns,
        entity_columns::set_columns,
        entity_columns::reset_columns,
    ),
    components(schemas(
        dashboard::TenantOverview,
        oauth::OAuthStatus,
        crate::models::usage::TenantUsage,
        crate::error::HostedApiErrorBody,
        crate::error::HostedApiErrorDetail,
        crate::models::marketplace::MarketplaceListing,
        crate::models::marketplace::TemplateVersionRecord,
        crate::models::marketplace::TemplateReviewRecord,
        crate::models::marketplace::PublishVersionRequest,
        crate::models::marketplace::SubmitReviewRequest,
        marketplace::ForkResponse,
        marketplace::SetVisibilityRequest,
        origin::MergeResponse,
        crate::services::merge::MergePlan,
        crate::services::merge::FieldMerge,
        crate::services::merge::MergeVerdict,
        yorishiro_core::models::schemas::UpstreamChange,
        yorishiro_core::models::schemas::SchemaRecord,
        yorishiro_core::metaschema::VersioningDiff,
    )),
    modifiers(&BearerKeySecurity),
    info(
        title = "Yorishiro Hosted API",
        description = "The endpoints `yorishiro-server` adds on top of the embedded \
                       community edition: Stripe billing, the admin dashboard's tenant \
                       overview, and OAuth2/OIDC login. Everything the community edition \
                       itself serves is documented separately at `/api-docs/openapi.json`.",
    ),
)]
pub struct HostedApiDoc;

/// Declares the bearer scheme `tenant_overview` references.
/// utoipa has no attribute form for this: a security scheme has to be attached through a `Modify` implementation.
struct BearerKeySecurity;

impl utoipa::Modify for BearerKeySecurity {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};

        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer_key",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "An API key in the same format `POST /auth/login` issues.",
                    ))
                    .build(),
            ),
        );
    }
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/mod.rs"]
mod tests;
