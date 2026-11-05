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
        let [created_at, updated_at] = helpers::timestamps(manager);
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("template_templates"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(
                        helpers::json_col(manager, Alias::new("tags"))
                            .not_null()
                            .default("[]"),
                    )
                    .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(ColumnDef::new(Alias::new("description")).text())
                    .col(helpers::json_col(manager, Alias::new("definition")).not_null())
                    .col(ColumnDef::new(Alias::new("locale")).text())
                    .col(
                        ColumnDef::new(Alias::new("visibility"))
                            .text()
                            .not_null()
                            .default("tenant"),
                    )
                    .col(ColumnDef::new(Alias::new("author")).text())
                    .col(helpers::uuid_col(manager, Alias::new("fork_of")))
                    .col(helpers::uuid_col(manager, Alias::new("created_by")))
                    .col(created_at)
                    .col(updated_at)
                    .check(Expr::cust("visibility IN ('tenant', 'community')"))
                    .index(
                        Index::create()
                            .name("template_templates_tenant_id_name_key")
                            .unique()
                            .col(Alias::new("tenant_id"))
                            .col(Alias::new("name")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_template_templates_tenant_id")
                            .from(Alias::new("template_templates"), Alias::new("tenant_id"))
                            .to(Alias::new("tenant_tenants"), Alias::new("id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_template_templates_fork_of")
                            .from(Alias::new("template_templates"), Alias::new("fork_of"))
                            .to(Alias::new("template_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_template_templates_created_by")
                            .from(Alias::new("template_templates"), Alias::new("created_by"))
                            .to(Alias::new("user_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("template_templates_tenant_id_idx")
                    .table(Alias::new("template_templates"))
                    .col(Alias::new("tenant_id"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("template_templates"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
