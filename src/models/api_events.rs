//! The durable, append-only trail of every REST request and MCP call.
//!
//! A call that can change something leaves a `started` row before it runs and an `authenticated` row once it has an identity.
//! Every call leaves a `completed` row when it ends. The rows of one call share a `request_id`.
//! An `authenticated` row with no `completed` row marks a write that was interrupted, and whose it was.
//! The application only appends and lists rows.
//! They hold identifiers and outcomes only: no credentials, bodies, query values or tool arguments.

pub use crate::models::_entities::api_events::{ActiveModel, Entity, Model};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, entity::prelude::*,
};

use crate::error::{ResultExt, YorishiroError};
use crate::models::access_keys::AuthContext;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        Ok(this)
    }
}

/// What a call is, known before it is authenticated.
#[derive(Clone)]
pub(crate) struct Call {
    pub request_id: String,
    pub transport: &'static str,
    pub event_name: String,
    pub read_only: bool,
    pub source_ip: Option<String>,
    pub user_agent: String,
}

/// How a call ended, with whoever it was authenticated as.
pub(crate) struct Completion {
    pub caller: Option<AuthContext>,
    pub permissions: Vec<String>,
    pub outcome: &'static str,
    pub status_code: Option<i64>,
    pub latency_ms: i64,
    pub resources: Vec<serde_json::Value>,
}

impl Call {
    fn active(&self) -> ActiveModel {
        ActiveModel {
            request_id: Set(self.request_id.clone()),
            transport: Set(self.transport.to_owned()),
            event_name: Set(self.event_name.clone()),
            read_only: Set(self.read_only),
            source_ip: Set(self.source_ip.clone()),
            user_agent: Set(self.user_agent.chars().take(256).collect()),
            identity_type: Set("anonymous".to_owned()),
            correlation_id: Set(self.request_id.clone()),
            resources: Set(serde_json::json!([])),
            permissions: Set(serde_json::json!([])),
            ..Default::default()
        }
    }

    /// Records that the call is about to run.
    pub(crate) async fn start(&self, db: &DatabaseConnection) -> Result<(), YorishiroError> {
        ActiveModel {
            phase: Set("started".to_owned()),
            ..self.active()
        }
        .insert(db)
        .await
        .internal()?;
        Ok(())
    }

    /// Records who the call turned out to be, so an interrupted write still belongs to a tenant.
    pub(crate) async fn authenticated(
        &self,
        db: &DatabaseConnection,
        actor: &AuthContext,
    ) -> Result<(), YorishiroError> {
        ActiveModel {
            phase: Set("authenticated".to_owned()),
            tenant_id: Set(Some(actor.tenant_id)),
            access_key_id: Set(Some(actor.access_key_id)),
            user_id: Set(actor.user_id),
            workspace_id: Set(actor.workspace_id),
            caller: Set(Some(actor.yrn())),
            identity_type: Set("access_key".to_owned()),
            ..self.active()
        }
        .insert(db)
        .await
        .internal()?;
        Ok(())
    }

    /// Records how the call ended.
    pub(crate) async fn finish(
        &self,
        db: &DatabaseConnection,
        completion: &Completion,
    ) -> Result<(), YorishiroError> {
        let caller = completion.caller.as_ref();
        ActiveModel {
            phase: Set("completed".to_owned()),
            tenant_id: Set(caller.map(|actor| actor.tenant_id)),
            access_key_id: Set(caller.map(|actor| actor.access_key_id)),
            user_id: Set(caller.and_then(|actor| actor.user_id)),
            workspace_id: Set(caller.and_then(|actor| actor.workspace_id)),
            caller: Set(caller.map(AuthContext::yrn)),
            identity_type: Set(if caller.is_some() {
                "access_key"
            } else {
                "anonymous"
            }
            .to_owned()),
            resources: Set(serde_json::json!(completion.resources)),
            permissions: Set(serde_json::json!(completion.permissions)),
            outcome: Set(Some(completion.outcome.to_owned())),
            status_code: Set(completion
                .status_code
                .and_then(|code| i32::try_from(code).ok())),
            latency_ms: Set(Some(completion.latency_ms)),
            ..self.active()
        }
        .insert(db)
        .await
        .internal()?;
        Ok(())
    }
}

impl Call {
    /// The end of a background job, recorded under the request that started it.
    pub(crate) const fn job(
        request_id: String,
        job_id: Uuid,
        job_name: String,
        workspace_id: Option<Uuid>,
        outcome: &'static str,
        attempt: i32,
    ) -> JobEnd {
        JobEnd {
            request_id,
            job_id,
            job_name,
            workspace_id,
            outcome,
            attempt,
        }
    }
}

/// How a background job ended.
pub(crate) struct JobEnd {
    request_id: String,
    job_id: Uuid,
    job_name: String,
    workspace_id: Option<Uuid>,
    outcome: &'static str,
    attempt: i32,
}

impl JobEnd {
    pub(crate) async fn append(self, db: &impl ConnectionTrait) -> Result<(), YorishiroError> {
        let tenant_id = match self.workspace_id {
            Some(workspace_id) => Some(
                crate::models::workspace_workspaces::get_workspace_tenant(db, workspace_id).await?,
            ),
            None => None,
        };
        ActiveModel {
            correlation_id: Set(self.request_id.clone()),
            request_id: Set(self.request_id),
            phase: Set("completed".to_owned()),
            identity_type: Set("service".to_owned()),
            caller: Set(Some(format!("yrn:yorishiro:service/{}", self.job_name))),
            resources: Set(serde_json::json!([])),
            attempt: Set(Some(self.attempt)),
            error_code: Set((self.outcome != "success").then(|| self.outcome.to_owned())),
            tenant_id: Set(tenant_id),
            workspace_id: Set(self.workspace_id),
            job_id: Set(Some(self.job_id)),
            transport: Set("job".to_owned()),
            event_name: Set(self.job_name),
            read_only: Set(false),
            user_agent: Set(String::new()),
            permissions: Set(serde_json::json!([])),
            outcome: Set(Some(self.outcome.to_owned())),
            ..Default::default()
        }
        .insert(db)
        .await
        .internal()?;
        Ok(())
    }
}

impl Model {
    /// The tenant's trail, most recent first, optionally narrowed to one request.
    ///
    /// # Errors
    /// Rejects callers without `audit:ListApiEvents`.
    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        params: crate::models::pagination::ListParams,
        request_id: Option<&str>,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::_entities::api_events::Column;
        use crate::models::iam::{
            access,
            policy::{Action, Resource},
        };
        let txn = crate::db::begin_write(db).await.internal()?;
        access::require_iam_action(
            &txn,
            actor,
            Action::ListApiEvents,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let mut query = Entity::find().filter(Column::TenantId.eq(actor.tenant_id));
        if let Some(request_id) = request_id {
            query = query.filter(Column::RequestId.eq(request_id));
        }
        let events = query
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .limit(params.limit())
            .offset(params.offset())
            .all(&txn)
            .await
            .internal()?;
        txn.commit().await.internal()?;
        Ok(events)
    }
}

/// Notes an IAM or tenant change on the call that made it, so its completed row names what changed.
///
/// # Errors
/// Rejects a caller that is not a user, because a change must be attributable to one.
#[expect(
    clippy::unused_async,
    reason = "call sites keep the transaction-bound shape the audit table had"
)]
pub(crate) async fn record(
    _txn: &sea_orm::DatabaseTransaction,
    actor: &AuthContext,
    action: crate::models::iam::policy::Action,
    detail: serde_json::Value,
) -> Result<(), YorishiroError> {
    actor.user_id.ok_or(YorishiroError::Unauthenticated)?;
    crate::controllers::middleware::access_log::changed(action, &detail);
    Ok(())
}

/// Deletes the trail rows older than `cutoff` and returns how many went.
///
/// # Errors
/// Returns an error when the delete fails.
pub(crate) async fn purge_before(
    db: &DatabaseConnection,
    cutoff: chrono::DateTime<chrono::FixedOffset>,
) -> Result<u64, YorishiroError> {
    use crate::models::_entities::api_events::Column;
    Ok(Entity::delete_many()
        .filter(Column::CreatedAt.lt(cutoff))
        .exec(db)
        .await
        .internal()?
        .rows_affected)
}

/// Records one task run as a single completed row: who ran it, what ran and how it ended.
pub(crate) async fn record_task(
    db: &DatabaseConnection,
    name: &str,
    identity_type: &'static str,
    read_only: bool,
    failure: Option<&loco_rs::Error>,
    latency_ms: i64,
) {
    let request_id = Uuid::now_v7().to_string();
    let caller = if identity_type == "operator" {
        format!("yrn:yorishiro:operator/{}", crate::data::env::operator())
    } else {
        format!("yrn:yorishiro:service/{name}")
    };
    let row = ActiveModel {
        correlation_id: Set(request_id.clone()),
        request_id: Set(request_id),
        phase: Set("completed".to_owned()),
        transport: Set("task".to_owned()),
        event_name: Set(name.to_owned()),
        read_only: Set(read_only),
        user_agent: Set(String::new()),
        identity_type: Set(identity_type.to_owned()),
        caller: Set(Some(caller)),
        resources: Set(serde_json::json!([])),
        permissions: Set(serde_json::json!([])),
        outcome: Set(Some(
            if failure.is_none() {
                "success"
            } else {
                "failed"
            }
            .to_owned(),
        )),
        // The task's own message can carry what it was given, so the trail keeps only that it failed.
        error_code: Set(failure.map(|_| "task_failed".to_owned())),
        latency_ms: Set(Some(latency_ms)),
        ..Default::default()
    };
    if let Err(error) = row.insert(db).await {
        tracing::error!(%error, task = name, "the task run could not be recorded in the trail");
    }
}
