use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let table = Table::create()
            .table(Alias::new("workspace_memberships"))
            .col(helpers::uuidv7_pk(m))
            .col(helpers::uuid_col(m, Alias::new("workspace_id")).not_null())
            .col(helpers::uuid_col(m, Alias::new("user_id")).not_null())
            .col(ColumnDef::new(Alias::new("role")).text().not_null())
            .col(helpers::created_at(m))
            .foreign_key(
                ForeignKey::create()
                    .name("fk_workspace_memberships_workspace_id")
                    .from(
                        Alias::new("workspace_memberships"),
                        Alias::new("workspace_id"),
                    )
                    .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_workspace_memberships_user_id")
                    .from(Alias::new("workspace_memberships"), Alias::new("user_id"))
                    .to(Alias::new("user_users"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();
        helpers::create_table_with_checks(
            m,
            "workspace_memberships",
            table,
            &[(
                "workspace_memberships_role_check",
                "role IN ('admin', 'member', 'viewer')",
            )],
        )
        .await?;
        m.create_index(
            Index::create()
                .name("uq_workspace_memberships_workspace_id_user_id")
                .table(Alias::new("workspace_memberships"))
                .col(Alias::new("workspace_id"))
                .col(Alias::new("user_id"))
                .unique()
                .to_owned(),
        )
        .await?;
        m.create_index(
            Index::create()
                .name("idx_workspace_memberships_user_id")
                .table(Alias::new("workspace_memberships"))
                .col(Alias::new("user_id"))
                .to_owned(),
        )
        .await?;
        // Control-plane membership is read through the identity pool, never the tenant role.
        helpers::enable_rls_with_policy(
            m,
            "workspace_memberships",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.drop_table(
            Table::drop()
                .table(Alias::new("workspace_memberships"))
                .to_owned(),
        )
        .await
    }
}
