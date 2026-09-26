use std::sync::Arc;

use loco_rs::{
    Result,
    app::AppContext,
    bgworker::{BackgroundWorker, Queue},
    task::Tasks,
};

use crate::db::AppContextBackend;
use crate::workers::embedding_sync::WorkerClassResolver;

/// Installs the enterprise services that override base shared-store seams.
pub(crate) fn compose_context(ctx: &AppContext) {
    // An absent or invalid licence key warns and continues rather than failing boot.
    ctx.shared_store.insert(
        Arc::new(crate::ee::services::licence::LicenceState::from_env())
            as Arc<dyn crate::services::edition::EnterpriseEdition>,
    );

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
            crate::ee::services::tenant_auth::TenantScopedAuthenticator,
        ) as Arc<dyn crate::services::auth::Authenticator>);
    }

    ctx.shared_store.insert(
        Arc::new(crate::ee::services::embedding_resolver::EmbeddingKeyResolver)
            as Arc<dyn crate::services::embedding::WorkspaceEmbeddingResolver>,
    );
    ctx.shared_store.insert(Arc::new(
        crate::ee::services::worker_class_resolver::WorkerClassAssignmentResolver,
    ) as Arc<dyn WorkerClassResolver>);
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
    crate::ee::services::official_templates::ensure_official_tenant(&ctx.db).await?;
    Ok(())
}
