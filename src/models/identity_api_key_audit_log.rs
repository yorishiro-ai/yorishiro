//! Records of authorization-gated operations: who did it, when, against which workspace, and what.
//!
//! Starts with the operations `ApiKeyScope::Migration` already gates (`content_entities::undo_job`, `identity_maintenance::set`), not every write in the system: those are exactly the ones an operator most needs an after-the-fact record of, since a `Migration`-scoped key can rewrite stored data or take the whole deployment down for other callers.
//! A new audited operation is added by extending [`AuditAction`], not by writing ad hoc `INSERT`s elsewhere: `action`'s CHECK constraint (`migration/src/m20260823_100700_api_key_audit_log.rs`) only allows what this enum's `as_db_str()` can produce, so the two stay in lockstep by construction.
//!
//! Append-only by construction, not just convention: `yorishiro_app` holds `SELECT, INSERT` on this table and nothing else (see the migration's own comment), so there is no code path, correct or buggy, that can update or delete a row once written.

use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, QueryOrder, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

pub use super::_entities::identity_api_key_audit_log::{ActiveModel, Entity, Model};
use crate::error::{ResultExt, YorishiroError};

pub type IdentityApiKeyAuditLog = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        Ok(self)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

/// The closed set of operations this table records.
/// Matches `action`'s CHECK constraint string-for-string; a variant added here without a matching value in the constraint fails every insert at the database, not silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// `content_entities::undo_job`: a `Migration`-scoped batch undo, restoring every entity a job's snapshots cover.
    UndoMigrationJob,
    /// `identity_maintenance::set`: a `Migration`-scoped maintenance mode change.
    SetMaintenance,
}

impl AuditAction {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::UndoMigrationJob => "undo_migration_job",
            Self::SetMaintenance => "set_maintenance",
        }
    }
}

/// The acting key, for [`record`]: what an audited operation attributes itself to.
/// A thin, owned copy of the fields of `auth::AuthContext`/`auth::CreatedApiKey` that `record` actually needs, so this module doesn't have to depend on `services::auth` for a handful of UUIDs.
#[derive(Debug, Clone, Copy)]
pub struct AuditActor {
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    pub api_key_id: Uuid,
    pub user_id: Option<Uuid>,
}

/// Appends one row.
///
/// `conn` is deliberately generic over `ConnectionTrait` rather than fixed to a `DatabaseTransaction`: `undo_migration_job` records this on the same RLS-scoped transaction its effect lands on (so a rollback loses both together), while `set_maintenance` records it on `ctx.db`, the migration-role connection its own write already goes through (see `identity_maintenance::set`'s doc comment for why that write can't be RLS-scoped in the first place).
/// The RLS policy on this table is strict (matching `content_entities`), so a `conn` that hasn't set `app.current_workspace` to `actor.workspace_id` first (or isn't the migration role, which the policy doesn't apply to at all) gets a row silently rejected by the `USING` clause rather than an error; callers on the RLS-scoped path get this for free from `TenantDb::begin_for_workspace`, already run before `record` by the time an `Authorized<R>` handler calls it.
pub async fn record(
    conn: &impl ConnectionTrait,
    actor: AuditActor,
    action: AuditAction,
    detail: serde_json::Value,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        workspace_id: ActiveValue::Set(actor.workspace_id),
        tenant_id: ActiveValue::Set(actor.tenant_id),
        api_key_id: ActiveValue::Set(Some(actor.api_key_id)),
        user_id: ActiveValue::Set(actor.user_id),
        action: ActiveValue::Set(action.as_db_str().to_string()),
        detail: ActiveValue::Set(detail),
        ..Default::default()
    };
    active.insert(conn).await.internal()?;
    Ok(())
}

/// The workspace's audit trail, most recent first, for an `audit`-permission key to review.
/// Same 200-row page cap `content_entities::list`/`content_relations::list` use, for the same reason: an unbounded read against a table that only grows is a query nobody meant to run.
pub async fn list_for_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Model>, YorishiroError> {
    use super::_entities::identity_api_key_audit_log::Column;

    let limit = limit.clamp(1, 200);
    let offset = offset.max(0);

    Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_desc(Column::CreatedAt)
        .limit(limit as u64)
        .offset(offset as u64)
        .all(conn)
        .await
        .internal()
}
