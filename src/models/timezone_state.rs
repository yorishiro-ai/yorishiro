use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, DatabaseConnection, EntityTrait, QuerySelect, Set,
};
use std::time::Duration;

pub use super::_entities::timezone_state::{ActiveModel, Entity};

const ACTIVATION_ATTEMPTS: u32 = 5;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(10);
const SINGLETON_ID: i32 = 1;

impl ActiveModelBehavior for ActiveModel {}

/// Stores the process-configured zone without changing any persisted timestamp instant.
///
/// # Errors
/// Returns an error if the configured zone is invalid or the state transition fails.
pub async fn activate(db: &DatabaseConnection, configured: &str) -> loco_rs::Result<()> {
    crate::time::parse_timezone(configured)
        .map_err(|_| loco_rs::Error::Message(format!("invalid IANA timezone {configured:?}")))?;
    for attempt in 0..ACTIVATION_ATTEMPTS {
        let result = activate_once(db, configured).await;
        match result {
            Err(error) if is_sqlite_busy(db, &error) && attempt + 1 < ACTIVATION_ATTEMPTS => {
                tokio::time::sleep(RETRY_BASE_DELAY * (1 << attempt)).await;
            }
            result => return result,
        }
    }
    unreachable!("activation retry loop returns on its final attempt")
}

async fn activate_once(db: &DatabaseConnection, configured: &str) -> loco_rs::Result<()> {
    let txn = crate::db::begin_write(db).await?;
    let current = Entity::find_by_id(SINGLETON_ID)
        .lock_exclusive()
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
