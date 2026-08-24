use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Independent of `scope`, deliberately: `scope` stays the four-way ordered read/write/schema/migration ladder (`ApiKeyScope`'s derived `Ord`), and folding audit into that ladder above `migration` would let an audit-reading key also run a batch migration and flip maintenance mode, which nobody asked for.
        // A separate boolean composes with any scope instead: a key is `scope=read, audit=true` to read the log without write access, or any other scope plus audit, entirely independent of where that scope sits in the ladder.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE identity_api_keys ADD COLUMN audit boolean NOT NULL DEFAULT false;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE identity_api_keys DROP COLUMN audit;")
            .await?;
        Ok(())
    }
}
