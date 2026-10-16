use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set, TransactionTrait};

pub use super::_entities::timezone_state::{ActiveModel, Entity};

/// Stores the process-configured zone without changing any persisted timestamp instant.
///
/// # Errors
/// Returns an error if the configured zone is invalid or the state transition fails.
pub async fn activate(db: &DatabaseConnection, configured: &str) -> loco_rs::Result<()> {
    crate::time::parse_timezone(configured)
        .map_err(|_| loco_rs::Error::Message(format!("invalid IANA timezone {configured:?}")))?;
    let txn = db.begin().await?;
    let current = Entity::find_by_id(1)
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
    tracing::info!(
        timezone = configured,
        "active timezone changed; stored instants were preserved"
    );
    txn.commit().await?;
    Ok(())
}
