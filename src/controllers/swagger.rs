//! OpenAPI documentation served through Swagger UI.
//!
//! Mount in `Hooks::after_routes`:
//! ```ignore
//! let router = swagger::mount(router);
//! ```
//! Exposes Swagger UI at `/docs` and the OpenAPI spec at `/docs/openapi.json`.

use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

/// Build the OpenAPI document.
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi()]
    struct ApiDoc;

    ApiDoc::openapi()
}

/// Mount Swagger UI onto the router.
pub fn mount(router: Router) -> Router {
    let swagger = SwaggerUi::new("/docs").url("/docs/openapi.json", openapi());
    router.merge(swagger)
}
