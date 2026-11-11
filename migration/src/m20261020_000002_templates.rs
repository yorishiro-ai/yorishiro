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
        Box::pin(up_templates(manager)).await?;
        Box::pin(up_template_catalog(manager)).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        Box::pin(down_template_catalog(manager)).await?;
        Box::pin(down_templates(manager)).await?;
        Ok(())
    }
}

async fn up_templates(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
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

async fn down_templates(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("template_templates"))
                .to_owned(),
        )
        .await?;

    Ok(())
}

async fn up_template_catalog(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
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

async fn down_template_catalog(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
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
