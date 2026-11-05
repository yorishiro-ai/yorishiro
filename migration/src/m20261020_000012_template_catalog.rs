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
            .create_table(
                Table::create()
                    .table(Alias::new("template_versions"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(helpers::uuid_col(manager, Alias::new("template_id")).not_null())
                    .col(ColumnDef::new(Alias::new("version")).integer().not_null())
                    .col(helpers::json_col(manager, Alias::new("definition")).not_null())
                    .col(ColumnDef::new(Alias::new("changelog")).text())
                    .col(
                        ColumnDef::new(Alias::new("status"))
                            .text()
                            .not_null()
                            .default("draft"),
                    )
                    .col(helpers::uuid_col(manager, Alias::new("created_by")))
                    .col(helpers::created_at(manager))
                    .check(Expr::cust("status IN ('draft', 'pre', 'stable')"))
                    .index(
                        Index::create()
                            .name("template_versions_template_id_version_key")
                            .unique()
                            .col(Alias::new("template_id"))
                            .col(Alias::new("version")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_template_versions_template_id")
                            .from(Alias::new("template_versions"), Alias::new("template_id"))
                            .to(Alias::new("template_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_template_versions_created_by")
                            .from(Alias::new("template_versions"), Alias::new("created_by"))
                            .to(Alias::new("user_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("template_versions_template_idx")
                    .table(Alias::new("template_versions"))
                    .col(Alias::new("template_id"))
                    .col((Alias::new("version"), IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("template_reviews"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(helpers::uuid_col(manager, Alias::new("template_id")).not_null())
                    .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                    .col(
                        ColumnDef::new(Alias::new("rating"))
                            .small_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("comment")).text())
                    .col(helpers::uuid_col(manager, Alias::new("created_by")))
                    .col(helpers::created_at(manager))
                    .col(
                        helpers::ts_tz_col(manager, Alias::new("updated_at"))
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .check(Expr::cust("rating BETWEEN 1 AND 5"))
                    .index(
                        Index::create()
                            .name("template_reviews_template_id_tenant_id_key")
                            .unique()
                            .col(Alias::new("template_id"))
                            .col(Alias::new("tenant_id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_template_reviews_template_id")
                            .from(Alias::new("template_reviews"), Alias::new("template_id"))
                            .to(Alias::new("template_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_template_reviews_tenant_id")
                            .from(Alias::new("template_reviews"), Alias::new("tenant_id"))
                            .to(Alias::new("tenant_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_template_reviews_created_by")
                            .from(Alias::new("template_reviews"), Alias::new("created_by"))
                            .to(Alias::new("user_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("template_reviews_template_idx")
                    .table(Alias::new("template_reviews"))
                    .col(Alias::new("template_id"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("template_reviews"))
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("template_versions"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
