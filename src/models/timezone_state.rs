use sea_orm::{
    AccessMode, ActiveModelBehavior, ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, QuerySelect, Set, SqliteTransactionMode, TransactionOptions, TransactionTrait,
};
use std::time::Duration;

pub use super::_entities::timezone_state::{ActiveModel, Entity};

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, _insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        Ok(self)
    }
}

/// Stores the process-configured zone without changing any persisted timestamp instant.
///
/// # Errors
/// Returns an error if the configured zone is invalid or the state transition fails.
pub async fn activate(db: &DatabaseConnection, configured: &str) -> loco_rs::Result<()> {
    crate::time::parse_timezone(configured)
        .map_err(|_| loco_rs::Error::Message(format!("invalid IANA timezone {configured:?}")))?;
    for attempt in 0..5 {
        let result = activate_once(db, configured).await;
        match result {
            Err(error) if is_sqlite_busy(db, &error) && attempt < 4 => {
                tokio::time::sleep(Duration::from_millis(10 << attempt)).await;
            }
            result => return result,
        }
    }
    unreachable!("activation retry loop returns on its final attempt")
}

async fn activate_once(db: &DatabaseConnection, configured: &str) -> loco_rs::Result<()> {
    let options = TransactionOptions {
        access_mode: Some(AccessMode::ReadWrite),
        sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
        ..Default::default()
    };
    let txn = db.begin_with_options(options).await?;
    let mut current_query = Entity::find_by_id(1);
    if db.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
        current_query = current_query.lock_exclusive();
    }
    let current = current_query
        .one(&txn)
        .await?
        .ok_or_else(|| loco_rs::Error::Message("timezone_state singleton row missing".into()))?;
    if current.active_timezone == configured {
        txn.commit().await?;
        return Ok(());
    }
    let mut active: ActiveModel = current.into();
    active.active_timezone = Set(configured.to_owned());
    active.update(&txn).await?;
    txn.commit().await?;
    tracing::info!(
        timezone = configured,
        "active timezone changed; stored instants were preserved"
    );
    Ok(())
}

fn is_sqlite_busy(db: &DatabaseConnection, error: &loco_rs::Error) -> bool {
    if db.get_database_backend() != sea_orm::DatabaseBackend::Sqlite {
        return false;
    }
    let mut source = Some(error as &(dyn std::error::Error + 'static));
    while let Some(error) = source {
        if let Some(sea_orm::sqlx::Error::Database(error)) =
            error.downcast_ref::<sea_orm::sqlx::Error>()
            && (error.code().as_deref() == Some("5") || error.code().as_deref() == Some("6"))
        {
            return true;
        }
        source = error.source();
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("database is locked") || message.contains("database is busy")
}
