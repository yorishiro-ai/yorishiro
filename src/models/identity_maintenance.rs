//! Deployment-wide maintenance state.
//!
//! One row, read on every request that could be refused and written only by an operator.
//! The request role has SELECT only on this table (`migration/src/m20260829_000000_initial_schema.rs`), so `set` runs on `ctx.db` (the migration-role connection), never the RLS-scoped tenant transaction.

pub use super::_entities::identity_maintenance::{ActiveModel, Entity, Model};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::{ResultExt, YorishiroError};

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// Stamps `updated_at` on every update whose caller didn't already set it explicitly.
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.updated_at = crate::db::stamped_updated_at(insert, this.updated_at);
        Ok(this)
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

/// What the deployment is currently refusing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceMode {
    /// Serving normally.
    Off,
    /// Reads served, writes refused with 423.
    ReadOnly,
    /// Everything refused with 503.
    FullLock,
}

impl MaintenanceMode {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::ReadOnly => "read_only",
            Self::FullLock => "full_lock",
        }
    }

    /// Parses the stored value.
    /// Unknown values are rejected rather than treated as `Off`: reading a row this crate does not understand and concluding "serve everything" would turn a corrupt row into an outage of the protection itself.
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "read_only" => Some(Self::ReadOnly),
            "full_lock" => Some(Self::FullLock),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MaintenanceState {
    pub mode: MaintenanceMode,
    pub retry_after: u32,
    pub reason: Option<String>,
}

impl MaintenanceState {
    /// The error to refuse a request with, or `None` when it may proceed.
    /// `is_write` decides whether read-only applies; full lock refuses either way.
    pub fn refusal(&self, is_write: bool) -> Option<YorishiroError> {
        let (refuse, read_only) = match self.mode {
            MaintenanceMode::Off => (false, false),
            MaintenanceMode::ReadOnly => (is_write, true),
            MaintenanceMode::FullLock => (true, false),
        };
        if !refuse {
            return None;
        }
        let message = self.reason.clone().unwrap_or_else(|| {
            if read_only {
                "the deployment is read-only for maintenance; writes are disabled".to_string()
            } else {
                "the deployment is unavailable for maintenance".to_string()
            }
        });
        Some(YorishiroError::Maintenance {
            message,
            read_only,
            retry_after: self.retry_after,
        })
    }
}

/// Reads the current state.
/// Runs on the request connection, so the row is readable by the application role.
pub async fn get(conn: &impl ConnectionTrait) -> Result<MaintenanceState, YorishiroError> {
    // Primary key is a boolean singleton (CHECK constraint enforces exactly TRUE).
    let row = Entity::find_by_id(true).one(conn).await.internal()?;

    // A missing row means off.
    // Not expected once the migration's seed row exists, but a missing row should be a survivable "off", not a panic.
    let Some(row) = row else {
        return Ok(MaintenanceState {
            mode: MaintenanceMode::Off,
            retry_after: 300,
            reason: None,
        });
    };

    let mode = MaintenanceMode::from_db_str(&row.mode).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "identity_maintenance.mode holds '{}', which is not a maintenance mode",
            row.mode
        ))
    })?;

    Ok(MaintenanceState {
        mode,
        retry_after: row.retry_after.max(1) as u32,
        reason: row.reason,
    })
}

/// Sets the state.
/// Takes the migration-role connection (`ctx.db`): the request role has SELECT only, since entering maintenance is an operator action.
pub async fn set(
    conn: &impl ConnectionTrait,
    mode: MaintenanceMode,
    retry_after: u32,
    reason: Option<String>,
) -> Result<MaintenanceState, YorishiroError> {
    let active = ActiveModel {
        id: sea_orm::ActiveValue::Unchanged(true),
        mode: sea_orm::ActiveValue::Set(mode.as_db_str().to_string()),
        retry_after: sea_orm::ActiveValue::Set(
            i32::try_from(retry_after.max(1)).unwrap_or(i32::MAX),
        ),
        reason: sea_orm::ActiveValue::Set(reason.clone()),
        ..Default::default()
    };
    active.update(conn).await.internal()?;

    Ok(MaintenanceState {
        mode,
        retry_after: retry_after.max(1),
        reason,
    })
}
