//! The durable, append-only trail of every REST request and MCP call.
//!
//! A call that can change something leaves a `started` row before it runs and an `authenticated` row once it has an identity.
//! Every call leaves a `completed` row when it ends. The rows of one call share a `request_id`.
//! An `authenticated` row with no `completed` row marks a write that was interrupted, and whose it was.
//! Rows are never updated or removed, which the database enforces with triggers.
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
