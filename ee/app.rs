//! `src/lib.rs` composes these contributions with the base application.
//! Plan-based quotas, worker and embedding lookups, route, worker, MCP tool and task registration, and seeding are overlaid here.

use std::sync::Arc;

use loco_rs::{Result, app::AppContext, task::Tasks};

use crate::app::RouteMounts;
use crate::controllers::mcp::McpToolSet;
use crate::data::tenant_policy::DEFAULT_TENANT_CAP;
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
    ctx: &AppContext,
    workspace_id: uuid::Uuid,
) -> std::result::Result<
    Option<Arc<dyn crate::services::embedding::EmbeddingProvider>>,
    crate::YorishiroError,
> {
    crate::edition::ee::models::workspace_embedding_keys::resolve(ctx, workspace_id).await
}

pub(crate) async fn workspace_embedding_metadata(
    conn: &sea_orm::DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> std::result::Result<Option<crate::services::embedding::EmbeddingMetadata>, crate::YorishiroError>
{
    crate::edition::ee::models::workspace_embedding_keys::metadata(conn, workspace_id).await
}

pub(crate) fn reindex_schedule(ctx: &AppContext) -> Result<Option<String>> {
    if !edition::is_active(ctx) {
        return Ok(None);
    }
    match ctx
        .config
        .initializers
        .as_ref()
        .and_then(|settings| settings.get("reindex_schedule"))
    {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(schedule)) => {
            let schedule = schedule.trim();
            Ok((!schedule.is_empty()).then(|| schedule.to_owned()))
        }
        Some(_) => Err(loco_rs::Error::Message(
            "initializers.reindex_schedule must be a cron string".into(),
        )),
    }
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
    if !licensed() && configured != DEFAULT_TENANT_CAP {
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
        Some(DEFAULT_TENANT_CAP)
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

/// Adds route groups after the base routes and applies the licence requirement declared by each group.
pub(crate) fn mount_routes(mounts: &mut RouteMounts, ctx: &AppContext) {
    let gate = axum::middleware::from_fn_with_state(ctx.clone(), edition::licence_gate);
    for routes in [
        controllers::embedding::routes(),
        controllers::entity_columns::routes(),
        controllers::inference::routes(),
        controllers::inference::gated_routes(),
        controllers::inference::inference_job_status_routes(),
        controllers::marketplace::routes(),
        controllers::oauth::routes(),
        controllers::origin::routes(),
        controllers::schema_forks::routes(),
        controllers::stripe::routes(),
        controllers::worker_class::routes(),
    ] {
        let routes = if routes.requires_licence() {
            routes.layer(gate.clone())
        } else {
            routes
        };
        mounts.mount(routes);
    }
    mounts
        .guard_credentials("/auth/oauth/authorize")
        .guard_credentials("/auth/oauth/callback");
}

/// Adds tools to the base MCP router.
pub(crate) fn mcp_tools(tools: McpToolSet) -> McpToolSet {
    tools.extend(
        controllers::mcp::tool_router(),
        controllers::mcp::tool_policy(),
    )
}

/// Registers enterprise tasks after the base tasks, preserving task-list order.
pub(crate) fn register_tasks(tasks: &mut Tasks) {
    tasks.register(crate::edition::ee::tasks::seed_official_templates::SeedOfficialTemplates);
    // tasks-inject (do not remove)
}

/// Performs the enterprise portion of application seeding.
pub(crate) async fn seed(ctx: &AppContext) -> Result<()> {
    crate::edition::ee::models::template_templates::ensure_official_tenant(&ctx.db).await?;
    Ok(())
}
