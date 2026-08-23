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
                    .table(Alias::new("identity_workspace_llm_keys"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("workspace_id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("base_url")).text().not_null())
                    .col(ColumnDef::new(Alias::new("model")).text().not_null())
                    .col(ColumnDef::new(Alias::new("api_key")).text().not_null())
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_workspace_llm_keys_workspace_id")
                            .from(
                                Alias::new("identity_workspace_llm_keys"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // No RLS and no GRANT, deliberately, matching identity_templates: yorishiro_app is
        // never the role that reaches this table. Reads and writes go through the
        // migration-role pool (ctx.db), which is what keeps a workspace's credentials off the
        // RLS-scoped request connection entirely rather than relying on a policy being right.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_workspace_llm_keys"))
                    .to_owned(),
            )
            .await
    }
}
