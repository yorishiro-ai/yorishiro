//! Compute credit ledger for a workspace.
//!
//! Append-only ledger of earn and spend events.
//! The balance is `SUM(amount)` over the rows, never a mutated counter.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection), not the RLS-scoped tenant pool, because `yorishiro_app` has no GRANT on this table.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::compute_credit_ledger::{ActiveModel, Column, Entity};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, ExprTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use serde::Serialize;
use uuid::Uuid;

/// A single row in the compute credit ledger.
#[derive(Clone, Serialize)]
pub struct LedgerEntry {
    pub workspace_id: Uuid,
    /// Positive for earn, negative for spend.
    pub amount: i64,
    pub transaction_type: TransactionType,
}

crate::models::db_enum::db_enum! {
    /// Which kind of ledger event this row records.
    #[derive(Clone, Copy, Eq, PartialEq)]
    pub enum TransactionType {
        Earn = "earn",
        Spend = "spend",
    }
}

/// Records an earn event for the workspace.
///
/// `amount` is stored as a positive value.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn earn(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    amount: i64,
) -> Result<(), YorishiroError> {
    if amount <= 0 {
        return Err(YorishiroError::ValidationFailed {
            message: "earn amount must be positive".into(),
            details: vec![],
            hint: "use spend for debit events".into(),
        });
    }
    insert_row(conn, workspace_id, amount, TransactionType::Earn).await
}

/// Records a spend event for the workspace.
///
/// `amount` is the positive quantity to deduct, and it is stored as a negative value.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn spend(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    amount: i64,
) -> Result<(), YorishiroError> {
    if amount <= 0 {
        return Err(YorishiroError::ValidationFailed {
            message: "spend amount must be positive".into(),
            details: vec![],
            hint: "use earn for credit events".into(),
        });
    }
    insert_row(conn, workspace_id, -amount, TransactionType::Spend).await
}

/// Inserts a row into the ledger.
///
/// The sign of `amount` encodes direction: positive for earn, negative for spend.
/// `earn` and `spend` own the sign, and this function does not check it.
async fn insert_row(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    amount: i64,
    transaction_type: TransactionType,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        amount: ActiveValue::Set(amount),
        transaction_type: ActiveValue::Set(transaction_type.as_db_str().to_string()),
        ..Default::default()
    };
    // `Entity::insert` skips model hooks, so run `before_save` for the id and timestamps.
    let active = sea_orm::ActiveModelBehavior::before_save(active, conn, true)
        .await
        .internal()?;
    Entity::insert(active).exec(conn).await.internal()?;
    Ok(())
}

/// Debits the positive actual cost after successful work without permitting an overdraft.
///
/// PostgreSQL serializes competing debits through the workspace lock; SQLite rejects a stale write transaction as a whole.
///
/// # Errors
/// Returns an error for nonpositive costs, insufficient balance or persistence failure.
pub async fn debit_actual(
    conn: &DatabaseTransaction,
    workspace_id: Uuid,
    amount: i64,
) -> Result<(), YorishiroError> {
    if amount <= 0 {
        return Err(YorishiroError::ValidationFailed {
            message: "actual credit cost must be positive".into(),
            details: vec![],
            hint: "debit only the positive cost of successful work".into(),
        });
    }
    crate::db::lock_for_update(conn, &format!("compute-credit:{workspace_id}"))
        .await
        .internal()?;
    if balance(conn, workspace_id).await? < amount {
        return Err(YorishiroError::ValidationFailed {
            message: "insufficient compute credits".into(),
            details: vec![],
            hint: "earn more compute credits before running this work".into(),
        });
    }
    insert_row(conn, workspace_id, -amount, TransactionType::Spend).await
}

/// Returns the workspace's current balance: `SUM(amount)` across all rows.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn balance(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<i64, YorishiroError> {
    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .select_only()
        .column_as(
            Expr::col(Column::Amount)
                .sum()
                .if_null(0)
                .cast_as(sea_orm::sea_query::Alias::new("bigint")),
            "total",
        )
        .into_tuple::<(Option<i64>,)>()
        .one(conn)
        .await
        .internal()?;
    Ok(rows.and_then(|(total,)| total).unwrap_or(0))
}

/// Returns the last `limit` entries for a workspace, most recent first.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
///
/// # Panics
/// Panics if an internal invariant required by this operation is violated.
pub async fn recent(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    limit: usize,
) -> Result<Vec<LedgerEntry>, YorishiroError> {
    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .order_by_desc(Column::CreatedAt)
        .limit(limit as u64)
        .all(conn)
        .await
        .internal()?;
    Ok(rows
        .into_iter()
        .map(|row| LedgerEntry {
            workspace_id: row.workspace_id,
            amount: row.amount,
            transaction_type: TransactionType::from_db_str(&row.transaction_type)
                .expect("CHECK constraint guarantees valid value"),
        })
        .collect())
}
