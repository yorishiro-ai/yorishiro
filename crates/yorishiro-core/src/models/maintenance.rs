//! Deployment-wide maintenance state.
//!
//! One row, read on every request that could be refused and written only by an operator.
//! It lives in the database rather than in the process because a flag held in memory would put one replica in maintenance while its siblings kept serving.

use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use utoipa::ToSchema;

use crate::error::{ResultExt, YorishiroError};

#[derive(Iden)]
enum Maintenance {
    Table,
    Mode,
    RetryAfter,
    Reason,
    UpdatedAt,
}

/// What the deployment is currently refusing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
    /// Unknown values are rejected rather than treated as `Off`:
    /// reading a row this crate does not understand and concluding "serve everything" would turn a corrupt row into an outage of the protection itself.
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "read_only" => Some(Self::ReadOnly),
            "full_lock" => Some(Self::FullLock),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
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

fn columns() -> [Maintenance; 3] {
    [
        Maintenance::Mode,
        Maintenance::RetryAfter,
        Maintenance::Reason,
    ]
}

/// Reads the current state.
/// Runs on the request connection, so the row is readable by the application role.
pub async fn get(conn: &mut PgConnection) -> Result<MaintenanceState, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(columns())
        .from((Alias::new("identity"), Maintenance::Table))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<(String, i32, Option<String>)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(&mut *conn)
        .await
        .internal()?;

    // A missing row means nobody has set maintenance, which is the same as off.
    let Some((mode, retry_after, reason)) = row else {
        return Ok(MaintenanceState {
            mode: MaintenanceMode::Off,
            retry_after: 300,
            reason: None,
        });
    };

    let mode = MaintenanceMode::from_db_str(&mode).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "identity.maintenance.mode holds '{mode}', which is not a maintenance mode"
        ))
    })?;

    Ok(MaintenanceState {
        mode,
        retry_after: retry_after.max(1) as u32,
        reason,
    })
}

/// Sets the state.
/// Takes the pool the migration role connects with: the request role has SELECT only, since entering maintenance is an operator action.
pub async fn set(
    pool: &PgPool,
    mode: MaintenanceMode,
    retry_after: u32,
    reason: Option<String>,
) -> Result<MaintenanceState, YorishiroError> {
    let (sql, values) = Query::update()
        .table((Alias::new("identity"), Maintenance::Table))
        .values([
            (Maintenance::Mode, mode.as_db_str().into()),
            (
                Maintenance::RetryAfter,
                i32::try_from(retry_after.max(1)).unwrap_or(i32::MAX).into(),
            ),
            (Maintenance::Reason, reason.clone().into()),
            (Maintenance::UpdatedAt, Expr::current_timestamp().into()),
        ])
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;

    Ok(MaintenanceState {
        mode,
        retry_after: retry_after.max(1),
        reason,
    })
}

#[cfg(test)]
#[path = "../../tests/models/maintenance.rs"]
mod tests;
