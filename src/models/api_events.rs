//! The durable, append-only trail of every REST request and MCP call.
//!
//! A write leaves a `started` row once it has an identity, before it runs.
//! Every call leaves a `completed` row when it ends. The rows of one call share a `request_id`.
//! A `started` row with no `completed` row marks a write that was interrupted, and whose it was.
//! The application only appends and lists rows.
//! They hold identifiers and outcomes only: no credentials, bodies, query values or tool arguments.

pub use crate::models::_entities::api_events::{ActiveModel, Entity, Model};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, QueryFilter,
    QueryOrder, QuerySelect, entity::prelude::*,
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

/// What narrows a listing of the trail.
#[derive(Default)]
pub(crate) struct Filter<'a> {
    pub request_id: Option<&'a str>,
    pub correlation_id: Option<&'a str>,
    pub resource: Option<&'a str>,
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
    /// What the call named, already reduced to identifiers and paging numbers by [`identifiers`].
    pub parameters: serde_json::Value,
}

/// How a call ended, with whoever it was authenticated as.
pub(crate) struct Completion {
    pub caller: Option<AuthContext>,
    pub permissions: Vec<String>,
    pub outcome: &'static str,
    pub status_code: Option<i64>,
    pub latency_ms: i64,
    pub resources: Vec<serde_json::Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
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
            request_parameters: Set(self.parameters.clone()),
            permissions: Set(serde_json::json!([])),
            ..Default::default()
        }
    }

    /// Records that an authenticated write is about to run, so an interrupted write still belongs to a tenant.
    pub(crate) async fn authenticated(
        &self,
        db: &DatabaseConnection,
        actor: &AuthContext,
    ) -> Result<(), YorishiroError> {
        ActiveModel {
            phase: Set("started".to_owned()),
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
            error_code: Set(completion.error_code.clone()),
            error_message: Set(completion.error_message.clone()),
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
        outcome: Option<&'static str>,
        attempt: i32,
    ) -> JobEnd {
        JobEnd {
            request_id,
            job_id,
            job_name,
            workspace_id,
            outcome,
            attempt,
            resources: serde_json::json!([]),
            error: None,
        }
    }
}

/// How a background job ended.
pub(crate) struct JobEnd {
    request_id: String,
    job_id: Uuid,
    job_name: String,
    workspace_id: Option<Uuid>,
    /// `None` for the row that marks an attempt starting.
    outcome: Option<&'static str>,
    attempt: i32,
    resources: serde_json::Value,
    error: Option<(String, String)>,
}

impl JobEnd {
    /// The code and fixed description of the error the attempt ended with.
    pub(crate) fn with_error(mut self, error: Option<(String, String)>) -> Self {
        self.error = error;
        self
    }

    /// Names the YRNs or identifiers the row is about, such as the entities a job failed on.
    pub(crate) fn with_resources(mut self, resources: serde_json::Value) -> Self {
        self.resources = resources;
        self
    }

    pub(crate) async fn append(self, db: &impl ConnectionTrait) -> Result<(), YorishiroError> {
        let tenant_id = match self.workspace_id {
            Some(workspace_id) => Some(
                crate::models::workspace_workspaces::get_workspace_tenant(db, workspace_id).await?,
            ),
            None => None,
        };
        let invoked_by = invoking_caller(db, &self.request_id).await;
        ActiveModel {
            invoked_by: Set(invoked_by),
            correlation_id: Set(self.request_id.clone()),
            request_id: Set(self.request_id),
            phase: Set(if self.outcome.is_some() {
                "completed"
            } else {
                "started"
            }
            .to_owned()),
            identity_type: Set("service".to_owned()),
            caller: Set(Some(format!("yrn:yorishiro:service/{}", self.job_name))),
            resources: Set(self.resources),
            request_parameters: Set(serde_json::json!({})),
            attempt: Set(Some(self.attempt)),
            error_code: Set(self
                .error
                .as_ref()
                .map(|error| error.0.clone())
                .or_else(|| {
                    self.outcome
                        .filter(|outcome| *outcome != "success")
                        .map(str::to_owned)
                })),
            error_message: Set(self.error.map(|error| error.1)),
            tenant_id: Set(tenant_id),
            workspace_id: Set(self.workspace_id),
            job_id: Set(Some(self.job_id)),
            transport: Set("job".to_owned()),
            event_name: Set(self.job_name),
            read_only: Set(false),
            user_agent: Set(String::new()),
            permissions: Set(serde_json::json!([])),
            outcome: Set(self.outcome.map(str::to_owned)),
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
        filter: Filter<'_>,
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
        if let Some(request_id) = filter.request_id {
            query = query.filter(Column::RequestId.eq(request_id));
        }
        if let Some(correlation_id) = filter.correlation_id {
            query = query.filter(Column::CorrelationId.eq(correlation_id));
        }
        if let Some(resource) = filter.resource {
            // The resources column holds identifiers only, so a substring match finds the one that is named.
            query = query.filter(
                Expr::col(Column::Resources)
                    .cast_as(sea_orm::sea_query::Alias::new("text"))
                    .like(format!("%{}%", resource.replace(['%', '_'], ""))),
            );
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
    crate::controllers::middleware::access_log::noted(action, &detail);
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
) -> Result<(), DbErr> {
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
        request_parameters: Set(serde_json::json!({})),
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
    row.insert(db).await.map(|_| ())
}

/// Reduces named values to what the trail may keep: UUIDs under `*_id` keys and the paging numbers.
/// Everything else, such as entity data, query text, names and definitions, is dropped, so the trail cannot hold content.
pub(crate) fn identifiers<'a>(
    pairs: impl IntoIterator<Item = (&'a str, &'a serde_json::Value)>,
) -> serde_json::Value {
    let kept = pairs
        .into_iter()
        .filter(|(key, value)| match *key {
            "limit" | "offset" => value.is_u64(),
            key if key.ends_with("_id") => value
                .as_str()
                .is_some_and(|text| Uuid::parse_str(text).is_ok()),
            _ => false,
        })
        .map(|(key, value)| (key.to_owned(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    serde_json::Value::Object(kept)
}

/// Who started the request a job belongs to: the caller on that request's first row that has one.
/// A job a scheduler or another job started has no such request, so its `invoked_by` stays empty.
async fn invoking_caller(db: &impl ConnectionTrait, request_id: &str) -> Option<String> {
    use crate::models::_entities::api_events::Column;
    Entity::find()
        .select_only()
        .column(Column::Caller)
        .filter(Column::RequestId.eq(request_id))
        .filter(Column::Transport.ne("job"))
        .filter(Column::Caller.is_not_null())
        .order_by_asc(Column::CreatedAt)
        .order_by_asc(Column::Id)
        .into_tuple::<String>()
        .one(db)
        .await
        .ok()
        .flatten()
}
