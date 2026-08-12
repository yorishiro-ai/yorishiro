mod entities;
mod export;
pub(crate) mod health;
mod identity;
mod import;
mod inference;
mod members;
mod relations;
mod schemas;
mod search;
mod setup;
mod template_library;
pub(crate) mod whoami;
mod workspaces;

use axum::Router;
use axum::routing::{get, post, put};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::repositories::tenancy::{self, MembershipRole};

use crate::state::AppState;

/// Parses a JSON-object query parameter (e.g. `?filter={"status":"active"}`) shared by the
/// `entities` and `search` list endpoints. `None`/empty input means "no filter".
pub(crate) fn parse_filter_param(
    raw: Option<String>,
) -> Result<Option<serde_json::Value>, YorishiroError> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    serde_json::from_str(&raw).map_err(|err| YorishiroError::ValidationFailed {
        message: "filter is not valid JSON".into(),
        details: vec![],
        hint: format!("filter must be a JSON object, e.g. {{\"status\":\"active\"}}: {err}"),
    })
}

/// Shared by `members` and `workspaces`: both are tenant-wide concerns, independent of (and
/// stricter than) the presented API key's own scope -- a Member-role key can carry `write`
/// scope for content operations while still having no business adding members or managing
/// workspaces.
pub(crate) async fn require_tenant_admin(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<(), YorishiroError> {
    let user_id = user_id.ok_or(YorishiroError::Unauthenticated)?;
    tenancy::get_membership_role(&state.identity_pool, tenant_id, user_id)
        .await?
        .filter(|role| matches!(role, MembershipRole::Owner | MembershipRole::Admin))
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "this operation is restricted to tenant owners/admins".into(),
            hint: "ask a tenant owner to grant you the admin role".into(),
        })?;
    Ok(())
}

/// Registers a single scheme named `bearer_auth` for sending the API key as a
/// Bearer token. Individual `#[utoipa::path]` items don't carry `security(...)`;
/// this registration plus `ApiDoc`'s top-level `security` attribute apply it to
/// every endpoint at once.
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("yorishiro-api-key")
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(
        identity::signup,
        identity::login,
        setup::status,
        setup::setup,
        members::list_members,
        members::add_member,
        entities::create_entity,
        entities::get_entity,
        entities::update_entity,
        entities::delete_entity,
        entities::list_entities,
        entities::get_entity_context,
        entities::get_entity_drift,
        entities::migration_dry_run,
        entities::fill_defaults,
        entities::undo_migration_job,
        inference::set_llm_key,
        inference::get_llm_key,
        inference::delete_llm_key,
        inference::infer_fill,
        inference::list_proposals,
        inference::confirm_proposals,
        relations::create_relation,
        relations::get_relation,
        relations::delete_relation,
        relations::list_relations,
        relations::set_relation_status,
        schemas::list_schemas,
        schemas::get_active_schema,
        schemas::get_schema_by_id,
        schemas::create_schema,
        schemas::get_entity_type_json_schema,
        schemas::list_templates,
        schemas::get_template,
        search::search_entities,
        export::export_jsonl,
        import::import_jsonl,
        workspaces::list_workspaces,
        workspaces::create_workspace,
        workspaces::get_workspace,
        workspaces::delete_workspace,
        template_library::list_templates,
        template_library::get_template,
        template_library::create_template,
        template_library::update_template,
        template_library::delete_template,
        template_library::fork_template,
    ),
    components(schemas(
        identity::SignupRequest,
        identity::SignupResponse,
        identity::WorkspaceSummary,
        identity::LoginRequest,
        identity::LoginResponse,
        setup::SetupStatusResponse,
        setup::SetupRequest,
        setup::SetupResponse,
        members::AddMemberRequest,
        yorishiro_core::repositories::tenancy::MembershipRole,
        yorishiro_core::repositories::tenancy::MembershipRecord,
        yorishiro_core::repositories::tenancy::WorkspaceRecord,
        yorishiro_core::repositories::entities::EntityDrift,
        yorishiro_core::repositories::entities::DriftField,
        yorishiro_core::repositories::entities::MigrationDryRun,
        yorishiro_core::repositories::entities::DryRunByType,
        yorishiro_core::repositories::entities::FillDefaultsReport,
        yorishiro_core::repositories::entities::UndoReport,
        yorishiro_core::services::auth::ApiKeyScope,
        entities::CreateEntityRequest,
        entities::UpdateEntityRequest,
        relations::CreateRelationRequest,
        relations::SetRelationStatusRequest,
        schemas::CreateSchemaResponse,
        schemas::CreateSchemaRequest,
        yorishiro_core::repositories::import::ImportResult,
        workspaces::CreateWorkspaceRequest,
        workspaces::WorkspaceDetail,
        yorishiro_core::repositories::tenancy::TemplateRecord,
        template_library::CreateTemplateRequest,
        template_library::UpdateTemplateRequest,
        template_library::ForkTemplateRequest,
    )),
    modifiers(&SecurityAddon),
    security(("bearer_auth" = [])),
    tags(
        (name = "auth", description = "Signup and login (no bearer token required)"),
        (name = "members", description = "Tenant member management (owner/admin only)"),
        (name = "workspaces", description = "Workspace management (listing is open to any tenant member; create/delete are owner/admin only)"),
        (name = "entities", description = "Entity operations"),
        (name = "relations", description = "Relation operations"),
        (name = "schemas", description = "Meta-schema operations"),
        (name = "search", description = "Vector similarity search"),
        (name = "export", description = "Bulk data export/import"),
        (name = "template-library", description = "Tenant-scoped, DB-backed schema template library (create/delete are owner/admin only)"),
    ),
    info(
        title = "Yorishiro API",
        description = "REST API for a user-defined-schema, MCP-native knowledge store",
    ),
)]
pub struct ApiDoc;

/// REST API routing. Returned as `Router<AppState>` without state applied, so
/// that `main.rs` can merge in the MCP routes and SwaggerUi before calling
/// `with_state` at the end.
///
/// `rate_limiter` protects `/auth/signup`, `/auth/login`, `/setup`, and `/setup/status` --
/// this crate's own bearer-token-free endpoints, and therefore the ones an unauthenticated
/// caller can brute-force (invite tokens, passwords). A downstream crate that adds its own
/// unauthenticated routes (e.g. an OAuth login/callback pair) should rate-limit those too,
/// via `crate::http::middleware::rate_limit::apply_rate_limit_layer` -- pass this same `Arc`
/// to share one quota with these routes, or a fresh one for an independent quota. See
/// `crate::build_app`'s doc comment for the full downstream-integration example.
pub fn router(
    rate_limiter: std::sync::Arc<crate::http::middleware::rate_limit::RateLimiter>,
) -> Router<AppState> {
    let auth_routes = crate::http::middleware::rate_limit::apply_rate_limit_layer(
        Router::new()
            .route("/auth/signup", post(identity::signup))
            .route("/auth/login", post(identity::login))
            .route("/setup", post(setup::setup))
            .route("/setup/status", get(setup::status)),
        rate_limiter,
    );

    Router::new()
        .merge(auth_routes)
        .route(
            "/api/members",
            post(members::add_member).get(members::list_members),
        )
        .route(
            "/api/workspaces",
            post(workspaces::create_workspace).get(workspaces::list_workspaces),
        )
        .route(
            "/api/workspaces/{id}",
            get(workspaces::get_workspace).delete(workspaces::delete_workspace),
        )
        .route(
            "/api/entities",
            post(entities::create_entity).get(entities::list_entities),
        )
        .route(
            "/api/entities/{id}",
            get(entities::get_entity)
                .put(entities::update_entity)
                .delete(entities::delete_entity),
        )
        .route(
            "/api/entities/{id}/context",
            get(entities::get_entity_context),
        )
        .route("/api/entities/{id}/drift", get(entities::get_entity_drift))
        .route(
            "/api/schemas/active/{name}/migration-dry-run",
            get(entities::migration_dry_run),
        )
        .route(
            "/api/schemas/active/{name}/fill-defaults",
            post(entities::fill_defaults),
        )
        .route(
            "/api/migration-jobs/{job_id}/undo",
            post(entities::undo_migration_job),
        )
        .route(
            "/api/workspace/llm-key",
            axum::routing::put(inference::set_llm_key)
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
        .route(
            "/api/relations",
            post(relations::create_relation).get(relations::list_relations),
        )
        .route(
            "/api/relations/{id}",
            get(relations::get_relation).delete(relations::delete_relation),
        )
        .route(
            "/api/relations/{id}/status",
            put(relations::set_relation_status),
        )
        .route(
            "/api/schemas",
            post(schemas::create_schema).get(schemas::list_schemas),
        )
        .route(
            "/api/schemas/active/{name}",
            get(schemas::get_active_schema),
        )
        .route(
            "/api/schemas/active/{name}/entity-types/{entity_type}/json-schema",
            get(schemas::get_entity_type_json_schema),
        )
        .route("/api/schemas/{schema_id}", get(schemas::get_schema_by_id))
        .route("/api/templates", get(schemas::list_templates))
        .route("/api/templates/{id}", get(schemas::get_template))
        .route(
            "/api/template-library",
            post(template_library::create_template).get(template_library::list_templates),
        )
        .route(
            "/api/template-library/{id}",
            get(template_library::get_template)
                .put(template_library::update_template)
                .delete(template_library::delete_template),
        )
        .route(
            "/api/template-library/{id}/fork",
            post(template_library::fork_template),
        )
        .route("/api/search", get(search::search_entities))
        .route("/api/export.jsonl", get(export::export_jsonl))
        .route("/api/import.jsonl", post(import::import_jsonl))
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/mod.rs"]
mod tests;
