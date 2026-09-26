use loco_rs::{
    Result,
    app::AppContext,
    bgworker::{BackgroundWorker, Queue},
    task::Tasks,
};

use crate::tasks;

/// Registers the base worker types in their existing queue order.
pub(super) async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
    use crate::workers::embedding_sync::{
        EmbeddingSyncWorkerOfficial, EmbeddingSyncWorkerShared, EmbeddingSyncWorkerTenantPrivate,
    };
    use crate::workers::reindex::{
        ReindexWorkerOfficial, ReindexWorkerShared, ReindexWorkerTenantPrivate,
    };

    queue
        .register(EmbeddingSyncWorkerTenantPrivate::build(ctx))
        .await?;
    queue
        .register(EmbeddingSyncWorkerOfficial::build(ctx))
        .await?;
    queue
        .register(EmbeddingSyncWorkerShared::build(ctx))
        .await?;
    queue
        .register(ReindexWorkerTenantPrivate::build(ctx))
        .await?;
    queue.register(ReindexWorkerOfficial::build(ctx)).await?;
    queue.register(ReindexWorkerShared::build(ctx)).await?;
    Ok(())
}

/// Registers the base tasks in their existing task-list order.
pub(super) fn register_tasks(tasks: &mut Tasks) {
    tasks.register(tasks::create_tenant::CreateTenant);
    tasks.register(tasks::create_workspace::CreateWorkspace);
    tasks.register(tasks::create_api_key::CreateApiKey);
    tasks.register(tasks::create_invite::CreateInvite);
    tasks.register(tasks::list_tenants::ListTenants);
    tasks.register(tasks::list_workspaces::ListWorkspaces);
    tasks.register(tasks::create_user::CreateUser);
    tasks.register(tasks::add_member::AddMember);
    tasks.register(tasks::list_members::ListMembers);
    tasks.register(tasks::list_api_keys::ListApiKeys);
    tasks.register(tasks::revoke_api_key::RevokeApiKey);
    tasks.register(tasks::resync_embeddings::ResyncEmbeddings);
    tasks.register(tasks::reindex_embeddings::ReindexEmbeddings);
    tasks.register(tasks::maintenance::Maintenance);
    tasks.register(tasks::maintenance_status::MaintenanceStatus);
    tasks.register(tasks::db_load_guard::DbLoadGuard);
}
