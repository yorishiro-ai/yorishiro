use std::sync::Arc;

use loco_rs::{Result, app::AppContext, task::Tasks};

use crate::app::RouteMounts;
use crate::controllers::mcp::McpToolSet;
use crate::edition::ee::controllers;
use crate::edition::ee::controllers::middleware::edition;
use crate::edition::ee::workers::infer_fill::{InferFillArgs, InferFillWorker};
use crate::workers::registry::WorkerRegistry;

pub(crate) async fn workspace_worker_class(
    conn: &sea_orm::DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> std::result::Result<Option<crate::workers::embedding_sync::WorkerClass>, crate::YorishiroError>
{
    crate::edition::ee::models::workspace_worker_classes::get(conn, workspace_id).await
}

pub(crate) async fn workspace_embedding_provider(
    conn: &sea_orm::DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> std::result::Result<
    Option<Arc<dyn crate::services::embedding::EmbeddingProvider>>,
    crate::YorishiroError,
> {
    crate::edition::ee::models::workspace_embedding_keys::resolve(conn, workspace_id).await
}

pub(crate) async fn workspace_embedding_metadata(
    conn: &sea_orm::DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> std::result::Result<Option<crate::services::embedding::EmbeddingMetadata>, crate::YorishiroError>
{
    crate::edition::ee::models::workspace_embedding_keys::metadata(conn, workspace_id).await
}

pub(crate) fn is_active(ctx: &AppContext) -> bool {
    edition::is_active(ctx)
}

pub(crate) async fn tenant_reindex_scheduler(
    ctx: &AppContext,
    tick: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    if is_active(ctx) {
        crate::edition::ee::models::tenant_reindex_schedules::tick(ctx, tick).await
    } else {
        crate::workers::startup_reindex::run_metadata_scan(ctx).await
    }
}

pub(crate) fn tenant_creation_cap(
    configured: i32,
    ctx: &AppContext,
) -> impl Fn() -> Option<i32> + Send + Sync + 'static {
    let licence = ctx
        .shared_store
        .get::<Arc<crate::edition::ee::controllers::middleware::edition::LicenceState>>();
    if licence.as_ref().is_none_or(|state| !state.is_active()) && configured != 1 {
        tracing::warn!(
            "YORISHIRO_MAX_TENANTS is ignored without a valid enterprise licence; enforcing one tenant"
        );
    }
    move || {
        tenant_cap_for_licence(
            configured,
            licence.as_ref().is_some_and(|state| state.is_active()),
        )
    }
}

pub(crate) fn tenant_cap_for_licence(configured: i32, licensed: bool) -> Option<i32> {
    if licensed {
        (configured > 0).then_some(configured)
    } else {
        Some(1)
    }
}

/// Installs the enterprise services that override base shared-store seams.
pub(crate) fn compose_context(ctx: &AppContext) -> Result<()> {
    crate::edition::ee::initializers::queue::install(ctx)?;
    crate::edition::ee::initializers::auth::install(ctx);
    Ok(())
}

/// Adds the enterprise workers after the base's, so their tags and queue come from the worker type like every other worker's.
pub(crate) fn workers(registry: WorkerRegistry) -> WorkerRegistry {
    registry.register::<InferFillArgs, InferFillWorker>()
        .register::<crate::edition::ee::workers::confirm_proposals::ConfirmProposalsArgs, crate::edition::ee::workers::confirm_proposals::ConfirmProposalsWorker>()
}

/// Mounts the enterprise edition's route groups after the community ones: `ee/` adds paths rather than replacing any.
///
/// `marketplace`, `stripe`, `oauth` and `inference::gated_routes` take the licence gate, and the rest serve without a licence; `.agents/rules/editions.md` records why each falls where it does, and widening that set is a product decision rather than something to inherit from where a layer is attached.
///
/// `stripe` and `oauth` are gated because billing and SSO login are enterprise-edition features, which is a decision about what each feature is rather than about what protects it.
/// Both still need their own configuration to do anything, and the webhook still verifies its Stripe signature; neither of those made them community routes.
///
/// `inference` is two route groups because the licence line runs through the middle of it:
/// `infer-fill` spends an LLM call and is gated, while the `/workspace/llm-key` routes beside it only store the credential.
/// A layer applies to a whole `Routes`, so one group would gate all four.
pub(crate) fn mount_routes(mounts: &mut RouteMounts, ctx: &AppContext) {
    let gate = axum::middleware::from_fn_with_state(ctx.clone(), edition::licence_gate);

    mounts
        .mount(controllers::dashboard::routes())
        .document(controllers::dashboard::openapi_docs());
    mounts
        .mount(controllers::embedding::routes())
        .document(controllers::embedding::openapi_docs());
    mounts
        .mount(controllers::entity_columns::routes())
        .document(controllers::entity_columns::openapi_docs());
    mounts
        .mount(controllers::inference::routes())
        .document(controllers::inference::openapi_docs());
    mounts
        .mount(controllers::inference::gated_routes().layer(gate.clone()))
        .document(edition::licence_required(
            controllers::inference::gated_openapi_docs(),
        ));
    mounts
        .mount(controllers::inference::inference_job_status_routes().layer(gate.clone()))
        .document(edition::licence_required(
            controllers::inference::job_status_openapi_docs(),
        ));
    mounts
        .mount(controllers::marketplace::routes().layer(gate.clone()))
        .document(edition::licence_required(
            controllers::marketplace::openapi_docs(),
        ));
    // `/auth/oauth/status` is deliberately not guarded: it carries no secret and the login page polls it on every load.
    mounts
        .mount(controllers::oauth::routes().layer(gate.clone()))
        .document(edition::licence_required(controllers::oauth::openapi_docs()))
        .guard_credentials("/auth/oauth/authorize")
        .guard_credentials("/auth/oauth/callback")
        .guard_credentials("/auth/oauth/invites/accept");
    mounts
        .mount(controllers::origin::routes())
        .document(controllers::origin::openapi_docs());
    mounts
        .mount(controllers::schema_forks::routes())
        .document(controllers::schema_forks::openapi_docs());
    mounts
        .mount(controllers::stripe::routes().layer(gate))
        .document(edition::licence_required(
            controllers::stripe::openapi_docs(),
        ));
    mounts
        .mount(controllers::worker_class::routes())
        .document(controllers::worker_class::openapi_docs());
}

/// Adds the enterprise MCP tools, and gates them on the licence.
pub(crate) fn mcp_tools(tools: McpToolSet) -> McpToolSet {
    tools.extend(
        controllers::mcp::tool_router(),
        controllers::mcp::tool_policy(),
    )
}

/// Registers enterprise tasks after the base tasks, preserving task-list order.
pub(crate) fn register_tasks(tasks: &mut Tasks) {
    tasks.register(crate::edition::ee::tasks::seed_official_templates::SeedOfficialTemplates);
    tasks.register(crate::edition::ee::tasks::create_tenant_api_key::CreateTenantApiKey);
    tasks.register(crate::edition::ee::tasks::sqlite_ann_benchmark::SqliteAnnBenchmark);
    // tasks-inject (do not remove)
}

/// Performs the enterprise portion of application seeding.
pub(crate) async fn seed(ctx: &AppContext) -> Result<()> {
    crate::edition::ee::models::template_templates::ensure_official_tenant(&ctx.db).await?;
    Ok(())
}
