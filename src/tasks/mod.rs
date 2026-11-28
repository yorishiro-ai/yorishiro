//! CLI tasks run through `cargo loco task`: maintenance, tenant retirement, embedding resync and credential recovery.

// Tenant retirement: inspect, disable, delete.
pub(crate) mod delete_due_tenants;
pub mod delete_tenant;
pub mod disable_tenant;
pub mod inspect_tenant_retirement;

// Accounts and credentials.
pub(crate) mod delete_user;
pub(crate) mod reset_root_password;

// Embeddings and the queue.
pub(crate) mod embedding_index_maintenance;
pub mod purge_audit_events;
pub(crate) mod recover_worker_dispatches;
pub mod reindex_scheduler;
pub(crate) mod resync_embeddings;

// Database and system maintenance.
pub(crate) mod db_load_guard;
pub mod maintenance;
pub(crate) mod maintenance_status;

/// Parses the target and explicit confirmation used by destructive tenant commands.
fn tenant_confirmation(vars: &loco_rs::task::Vars) -> loco_rs::Result<(uuid::Uuid, uuid::Uuid)> {
    let parse = |name: &str| -> loco_rs::Result<uuid::Uuid> {
        vars.cli_arg(name)?.parse::<uuid::Uuid>().map_err(|_| {
            crate::YorishiroError::ValidationFailed {
                message: format!("{name} is not a valid UUID"),
                details: vec![],
                hint: "provide matching tenant_id and confirmed_tenant_id UUIDs".into(),
            }
            .into()
        })
    };
    Ok((parse("tenant_id")?, parse("confirmed_tenant_id")?))
}

/// Runs a task and records the run in the trail, as the service that schedules it or the operator who typed it.
pub(crate) struct Audited<T> {
    task: T,
    identity_type: &'static str,
    read_only: bool,
}

impl<T> Audited<T> {
    pub(crate) const fn service(task: T) -> Self {
        Self {
            task,
            identity_type: "service",
            read_only: false,
        }
    }

    pub(crate) const fn operator(task: T) -> Self {
        Self {
            task,
            identity_type: "operator",
            read_only: false,
        }
    }

    pub(crate) const fn reading(mut self) -> Self {
        self.read_only = true;
        self
    }
}

#[loco_rs::prelude::async_trait]
impl<T: loco_rs::task::Task> loco_rs::task::Task for Audited<T> {
    fn task(&self) -> loco_rs::task::TaskInfo {
        self.task.task()
    }

    async fn run(
        &self,
        ctx: &loco_rs::app::AppContext,
        vars: &loco_rs::task::Vars,
    ) -> loco_rs::Result<()> {
        let started = std::time::Instant::now();
        let result = self.task.run(ctx, vars).await;
        crate::models::api_events::record_task(
            &ctx.db,
            &self.task.task().name,
            self.identity_type,
            self.read_only,
            result.as_ref().err(),
            i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
        )
        .await;
        result
    }
}
