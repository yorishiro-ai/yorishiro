use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("query_embedding_requests"))
                    .add_column(helpers::ts_tz_col(manager, Alias::new("deadline_at")))
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE query_embedding_requests SET status = 'expired' WHERE status = 'pending'",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        _manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("query_embedding_requests"))
                    .drop_column(Alias::new("deadline_at"))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
