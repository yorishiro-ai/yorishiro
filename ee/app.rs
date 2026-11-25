//! `src/lib.rs` composes these contributions with the base application.
//! Plan-based quotas, worker and embedding lookups, route, worker, MCP tool and task registration, and seeding are overlaid here.

use std::sync::Arc;

use loco_rs::{Result, app::AppContext, task::Tasks};

use crate::app::RouteMounts;
use crate::controllers::mcp::McpToolSet;
use crate::edition::ee::controllers;
use crate::edition::ee::controllers::middleware::edition;
use crate::edition::ee::workers::infer_fill::{InferFillArgs, InferFillWorker};
use crate::workers::registry::WorkerRegistry;

pub(crate) async fn quota_override(
    ctx: &AppContext,
    tenant_id: uuid::Uuid,
) -> std::result::Result<Option<crate::data::quotas::Quotas>, crate::YorishiroError> {
    use crate::edition::ee::data::plan::{Plan, PlanPolicy};
    if PlanPolicy::current(ctx)? == PlanPolicy::Disabled {
        return Ok(None);
    }
    let plan = Plan::for_tenant(ctx, tenant_id).await?;
    let caps = plan.caps();
    Ok(Some(crate::data::quotas::Quotas {
        workspaces: caps.max_workspaces,
        entities: caps.default_max_entities,
    }))
}

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

pub(crate) async fn run_reindex_schedule_override(
    ctx: &AppContext,
    tick: chrono::DateTime<chrono::Utc>,
) -> Result<bool> {
    if !is_active(ctx) {
        return Ok(false);
    }
    crate::edition::ee::models::tenant_reindex_schedules::tick(ctx, tick).await?;
    Ok(true)
}

pub(crate) fn tenant_creation_cap(
    configured: i32,
    ctx: &AppContext,
) -> impl Fn() -> Option<i32> + Send + Sync + 'static {
    let store = ctx.shared_store.clone();
    let licensed = move || {
        store
            .get::<Arc<edition::LicenceState>>()
            .is_some_and(|state| state.is_active())
    };
    if !licensed() && configured != 1 {
        tracing::warn!(
            "YORISHIRO_MAX_TENANTS is ignored without a valid enterprise licence; enforcing one tenant"
        );
    }
    move || tenant_cap_for_licence(configured, licensed())
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
    crate::edition::ee::initializers::licence::install(ctx);
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

    mounts.mount(controllers::dashboard::routes());
    mounts.mount(controllers::embedding::routes());
    mounts.mount(controllers::entity_columns::routes());
    mounts.mount(controllers::inference::routes());
    mounts.mount(
        controllers::inference::gated_routes()
            .licensed()
            .layer(gate.clone()),
    );
    mounts.mount(
        controllers::inference::inference_job_status_routes()
            .licensed()
            .layer(gate.clone()),
    );
    mounts.mount(
        controllers::marketplace::routes()
            .licensed()
            .layer(gate.clone()),
    );
    // OAuth status shares the licence gate with login entry points, including when OAuth is unconfigured.
    mounts
        .mount(controllers::oauth::routes().licensed().layer(gate.clone()))
        .guard_credentials("/auth/oauth/authorize")
        .guard_credentials("/auth/oauth/callback");
    mounts.mount(controllers::origin::routes());
    mounts.mount(controllers::schema_forks::routes());
    mounts.mount(controllers::stripe::routes().licensed().layer(gate));
    mounts.mount(controllers::worker_class::routes());
}

/// Adds origin MCP tools with the same ungated policy as their REST routes.
pub(crate) fn mcp_tools(tools: McpToolSet) -> McpToolSet {
    tools.extend(
        controllers::mcp::tool_router(),
        controllers::mcp::tool_policy(),
    )
}

/// Registers enterprise tasks after the base tasks, preserving task-list order.
pub(crate) fn register_tasks(tasks: &mut Tasks) {
    tasks.register(crate::edition::ee::tasks::seed_official_templates::SeedOfficialTemplates);
    tasks.register(crate::edition::ee::tasks::tenant_reindex_schedule::TenantReindexSchedule);
    // tasks-inject (do not remove)
}

/// Performs the enterprise portion of application seeding.
pub(crate) async fn seed(ctx: &AppContext) -> Result<()> {
    crate::edition::ee::models::template_templates::ensure_official_tenant(&ctx.db).await?;
    Ok(())
}
