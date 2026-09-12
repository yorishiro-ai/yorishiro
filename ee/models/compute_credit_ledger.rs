//! Compute credit ledger for a workspace.
//!
//! Append-only ledger tracking earn and spend events.
//! Balances are derived from `SUM(amount)` filtered by `transaction_type`
//! rather than mutated in place, so lost or reordered rows always produce
//! the correct total.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection),
//! not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this
//! table, matching `workspace_llm_keys` / `workspace_embedding_keys`.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::compute_credit_ledger::{ActiveModel, Column, Entity};
use sea_orm::ActiveModelBehavior;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::Serialize;
use uuid::Uuid;

/// A single row in the compute credit ledger.
#[derive(Debug, Clone, Serialize)]
pub struct LedgerEntry {
    pub workspace_id: Uuid,
    /// Positive for earn, negative for spend.
    pub amount: i64,
    pub transaction_type: TransactionType,
}

/// Which kind of ledger event this row records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TransactionType {
    Earn,
    Spend,
}

impl TransactionType {
    /// Serialize to the string stored in the database.
    #[must_use]
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Earn => "earn",
            Self::Spend => "spend",
        }
    }

    /// Parse from a database value.
    ///
    /// # Errors
    /// Returns an error if `value` is not one of the two known strings.
    pub fn from_db_str(value: &str) -> Result<Self, YorishiroError> {
        match value {
            "earn" => Ok(Self::Earn),
            "spend" => Ok(Self::Spend),
            other => Err(YorishiroError::Internal(anyhow::anyhow!(
                "unknown transaction_type value: {other:?}"
            ))),
        }
    }
}

/// Records an earn event for the workspace.
///
/// `amount` is stored as a positive value.
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
/// `amount` is the positive quantity to deduct; it is stored as a negative
/// value in the ledger so that `SUM(amount)` across all rows yields the
/// correct balance.
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
/// This function does not enforce that — callers `earn`/`spend` handle sign
/// logic so the database value always matches intent.
async fn insert_row(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    amount: i64,
    transaction_type: TransactionType,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        id: ActiveValue::NotSet, // uuidv7() default on PG; hooks on SQLite
        workspace_id: ActiveValue::Set(workspace_id),
        amount: ActiveValue::Set(amount),
        transaction_type: ActiveValue::Set(transaction_type.as_db_str().to_string()),
        created_at: ActiveValue::Set(chrono::Utc::now().into()),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
    };
    Entity::insert(active).exec(conn).await.internal()?;
    Ok(())
}

/// Returns the workspace's current balance: `SUM(amount)` across all rows.
///
/// Because earn rows are positive and spend rows are stored as negative,
/// the raw sum is the running balance in credit units.
pub async fn balance(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<i64, YorishiroError> {
    let rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .select_only()
        .column_as(Expr::cust("COALESCE(SUM(amount), 0)"), "total")
        .into_tuple::<(Option<i64>,)>()
        .one(conn)
        .await
        .internal()?;
    Ok(rows.and_then(|(total,)| total).unwrap_or(0))
}

/// Returns the last `limit` entries for a workspace, most recent first.
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

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}
