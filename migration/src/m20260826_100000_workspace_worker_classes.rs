use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let [created_at, updated_at] = helpers::timestamps();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_workspace_worker_classes"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("workspace_id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    // The same three-value string WorkerClass::as_db_str/from_db_str already round-trip
                    // (base's own serde(rename_all = "snake_case") wire form), so a row read here and a
                    // value read off a queued job's payload are byte-identical.
                    .col(ColumnDef::new(Alias::new("worker_class")).text().not_null())
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_workspace_worker_classes_workspace_id")
                            .from(
                                Alias::new("identity_workspace_worker_classes"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // No RLS and no GRANT, deliberately, matching identity_workspace_embedding_keys/
        // identity_workspace_llm_keys: yorishiro_app never reaches this table. Reads and writes go
        // through the migration-role pool (ctx.db), keeping which compute a workspace's jobs run on
        // off the RLS-scoped request connection entirely rather than relying on a policy being right.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_workspace_worker_classes"))
                    .to_owned(),
            )
            .await
    }
}
