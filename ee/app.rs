use std::sync::Arc;

use loco_rs::{
    Result,
    app::AppContext,
    bgworker::{BackgroundWorker, Queue},
    task::Tasks,
};

use crate::db::AppContextBackend;
use crate::workers::dispatch::LocoJobDispatcher;
use crate::workers::embedding_sync::WorkerClassResolver;
use crate::workers::queue::QueuePolicy;

/// Installs the enterprise services that override base shared-store seams.
pub(crate) fn compose_context(ctx: &AppContext) {
    // An absent or invalid licence key warns and continues rather than failing boot.
    let licence = Arc::new(crate::ee::controllers::middleware::edition::LicenceState::from_env());
    ctx.shared_store.insert(licence);

    if ctx.is_sqlite() {
        // These features use PostgreSQL-only SQL and report their limitation at boot.
        tracing::warn!(
            "some enterprise features are unavailable on SQLite: browsing the marketplace, publishing \
             a template version, and listing template-origin updates each run a PostgreSQL-only \
             query and will fail when reached. Point DATABASE_URL at PostgreSQL to use them; \
             vector search and everything else works on this backend"
        );
    } else {
        // Replaces the default authenticator installed by the base context builder.
        ctx.shared_store.insert(Arc::new(
            crate::ee::controllers::middleware::auth::TenantScopedAuthenticator,
        )
            as Arc<dyn crate::controllers::middleware::auth::Authenticator>);
    }

    ctx.shared_store.insert(Arc::new(
        crate::ee::models::workspace_embedding_keys::EmbeddingKeyResolver,
    )
        as Arc<dyn crate::services::embedding::WorkspaceEmbeddingResolver>);
    ctx.shared_store.insert(Arc::new(
        crate::ee::models::workspace_worker_classes::WorkerClassAssignmentResolver,
    ) as Arc<dyn WorkerClassResolver>);
    // Replaces the community queue policy installed by the base context builder.
    ctx.shared_store
        .insert(Arc::new(crate::ee::workers::queue::PlanQueuePolicy) as Arc<dyn QueuePolicy>);
    ctx.shared_store
        .insert(Arc::new(LocoJobDispatcher)
            as Arc<dyn crate::ee::workers::infer_fill::InferFillDispatcher>);
}

/// The named queues the enterprise workers enqueue to, which the base serves on Redis after its own.
/// The values here are also used by `all_tags()` to derive the enterprise tags, so tags and
/// queue lists share a single source of truth and cannot drift.
pub(crate) fn worker_queues() -> Vec<String> {
    vec![crate::ee::workers::infer_fill::QUEUE.to_owned()]
}

/// Registers enterprise workers after the base workers, preserving queue order.
pub(crate) async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
    queue
        .register(crate::ee::workers::infer_fill::InferFillWorker::build(ctx))
        .await?;
    Ok(())
}

/// Registers enterprise tasks after the base tasks, preserving task-list order.
pub(crate) fn register_tasks(tasks: &mut Tasks) {
    tasks.register(crate::ee::tasks::seed_official_templates::SeedOfficialTemplates);
    tasks.register(crate::ee::tasks::create_tenant_api_key::CreateTenantApiKey);
    tasks.register(crate::ee::tasks::reindex_scheduler::TenantReindexScheduler);
    tasks.register(crate::ee::tasks::sqlite_ann_benchmark::SqliteAnnBenchmark);
    // tasks-inject (do not remove)
}

/// Performs the enterprise portion of application seeding.
pub(crate) async fn seed(ctx: &AppContext) -> Result<()> {
    crate::ee::models::template_templates::ensure_official_tenant(&ctx.db).await?;
    Ok(())
}
